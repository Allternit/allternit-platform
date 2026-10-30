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


Call = TypedDict('Call', {'id': str, 'object': Literal['call'], 'account_id': str, 'agent_id': str, 'number_id': Optional[str], 'direction': Literal['inbound', 'outbound', 'realtime'], 'from': Optional[str], 'to': Optional[str], 'purpose': Optional[str], 'status': Literal['queued', 'ringing', 'in_progress', 'completed', 'failed', 'no_answer', 'canceled'], 'end_reason': Optional[str], 'simulated': bool, 'recording': bool, 'transferred_to': Optional[str], 'duration_seconds': Optional[int], 'created_at': str, 'answered_at': Optional[str], 'ended_at': Optional[str]}, total=False)
Error = TypedDict('Error', {'error': Dict[str, Any]}, total=False)
SpendCap = TypedDict('SpendCap', {'object': Literal['spend_cap'], 'project_id': str, 'plan': Literal['sandbox', 'payg', 'growth', 'enterprise'], 'spend_cap_cents': int, 'period': str, 'spent_cents': int, 'spent_microusd': int, 'remaining_cents': int, 'reached': bool, 'payment_method': bool, 'thresholds_sent': List[int]}, total=False)
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
ProjectTracing = TypedDict('ProjectTracing', {'object': Literal['project.tracing'], 'project_id': str, 'endpoint': str, 'header_names': List[str], 'service_name': str, 'enabled': bool, 'last_export_at': Optional[str], 'last_error': Optional[str], 'created_at': str, 'updated_at': str}, total=False)
UsageRow = TypedDict('UsageRow', {'group': Optional[str], 'meter': Literal['voice_min_allternit', 'voice_min_byok', 'agent_month', 'tokens_in', 'tokens_out', 'number_local_month', 'number_tollfree_month', 'sms_segment', 'mms', 'registration_passthrough_cents', 'recording_min_month', 'computer_minute', 'computer_action', 'mailbox_month', 'identity_month'], 'unit': Optional[str], 'quantity': float, 'events': int}, total=False)
PhoneNumber = TypedDict('PhoneNumber', {'id': str, 'object': Literal['phone_number'], 'account_id': Optional[str], 'e164': str, 'type': Literal['local', 'toll_free'], 'sms_state': Literal['pending_registration', 'active', 'rejected', 'blocked'], 'simulated': bool, 'created_at': str}, total=False)
PhoneNumberList = TypedDict('PhoneNumberList', {'data': List["PhoneNumber"], 'has_more': bool, 'next_cursor': Optional[str]}, total=False)
Message = TypedDict('Message', {'id': str, 'object': Literal['message'], 'account_id': Optional[str], 'number_id': str, 'direction': Literal['inbound', 'outbound'], 'from': str, 'to': str, 'body': str, 'segments': int, 'status': Literal['sent', 'simulated', 'received', 'failed'], 'error_code': Optional[str], 'media': List[Dict[str, Any]], 'created_at': str}, total=False)
MessageList = TypedDict('MessageList', {'data': List["Message"], 'has_more': bool, 'next_cursor': Optional[str]}, total=False)
WebhookEndpoint = TypedDict('WebhookEndpoint', {'id': str, 'object': Literal['webhook_endpoint'], 'url': str, 'events': List[str], 'description': Optional[str], 'created_at': str}, total=False)
Event = TypedDict('Event', {'id': str, 'object': Literal['event'], 'type': Literal['approval.requested', 'message.received', 'message.status', 'registration.updated', 'inbox.item.created', 'call.started', 'call.ended', 'call.transcript.ready', 'usage.threshold', 'phone.step', 'phone.run.completed', 'phone.fleet_run.completed', 'mail.received', 'webhook.test'], 'created': int, 'project_id': str, 'account_id': Optional[str], 'data': Dict[str, Any]}, total=False)
Computer = TypedDict('Computer', {'id': str, 'object': Literal['computer'], 'name': str, 'status': Literal['provisioning', 'starting', 'running', 'stopping', 'stopped', 'error', 'deleted'], 'account_id': Optional[str], 'key_id': str, 'created_at': str, 'started_at': Optional[str], 'metadata': Dict[str, Any]}, total=False)
ComputerList = TypedDict('ComputerList', {'data': List["Computer"], 'has_more': bool, 'next_cursor': Optional[str]}, total=False)
ComputerEvent = TypedDict('ComputerEvent', {'id': str, 'type': Literal['computer.action'], 'ts': str, 'data': Dict[str, Any]}, total=False)
ComputerEventList = TypedDict('ComputerEventList', {'data': List["ComputerEvent"], 'has_more': bool, 'next_cursor': Optional[str]}, total=False)
ComputerSettings = TypedDict('ComputerSettings', {'hosted_driver_enabled': bool, 'approval_mode': Literal['owner', 'api_key'], 'per_key_concurrency': int, 'browser_toolset': bool}, total=False)
ToolsetCall = TypedDict('ToolsetCall', {'toolset': Literal['computer', 'browser'], 'member': str, 'input': Dict[str, Any], 'run_id': str, 'turn_id': str, 'call_index': int, 'model_frame': Dict[str, Any], 'coordinate_space': Literal['pixels', 'normalized_1000'], 'approval_grant': str, 'browser_session_id': str, 'enable': List[str], 'agent_id': str, 'agent_name': str, 'step': str}, total=False)
ToolsetResult = TypedDict('ToolsetResult', {'is_error': bool, 'content': List[Dict[str, Any]], 'browser_state': Dict[str, Any], 'screen': Dict[str, Any], 'error': str}, total=False)
ToolsetApprovalRequired = TypedDict('ToolsetApprovalRequired', {'error': Dict[str, Any], 'approval': Dict[str, Any], 'result': "ToolsetResult"}, total=False)
Mailbox = TypedDict('Mailbox', {'id': str, 'object': Literal['mailbox'], 'account_id': str, 'agent_id': Optional[str], 'address': str, 'display_name': Optional[str], 'created_at': str}, total=False)
MailboxList = TypedDict('MailboxList', {'data': List["Mailbox"], 'has_more': bool, 'next_cursor': Optional[str]}, total=False)
MailMessage = TypedDict('MailMessage', {'id': str, 'object': Literal['mail_message'], 'mailbox_id': str, 'from': str, 'to': str, 'subject': Optional[str], 'snippet': Optional[str], 'codes': List[str], 'links': List[str], 'is_verification': bool, 'received_at': str}, total=False)
MailMessageList = TypedDict('MailMessageList', {'data': List["MailMessage"], 'has_more': bool, 'next_cursor': Optional[str]}, total=False)
Identity = TypedDict('Identity', {'id': str, 'object': Literal['identity'], 'account_id': str, 'agent_id': Optional[str], 'number': Union["PhoneNumber", Optional[Any]], 'mailbox': Union["Mailbox", Optional[Any]], 'phone_serial': Optional[str], 'bundle': Dict[str, Any], 'created_at': str}, total=False)
IdentityList = TypedDict('IdentityList', {'data': List["Identity"], 'has_more': bool, 'next_cursor': Optional[str]}, total=False)
Phone = TypedDict('Phone', {'object': Literal['phone'], 'serial': str, 'name': Optional[str], 'model': Optional[str], 'online': bool, 'desktop_online': bool, 'last_seen_at': Optional[str], 'call_bridge': Optional[Dict[str, Any]]}, total=False)
PhoneList = TypedDict('PhoneList', {'data': List["Phone"], 'has_more': bool, 'next_cursor': Optional[str]}, total=False)
PhoneSkillList = TypedDict('PhoneSkillList', {'data': List[Dict[str, Any]], 'has_more': bool, 'next_cursor': Optional[str]}, total=False)
PhoneStep = TypedDict('PhoneStep', {'object': Literal['phone.step'], 'id': str, 'run_id': str, 'serial': str, 'at': str, 'tool': str, 'label': str, 'args': Dict[str, Any], 'ok': bool, 'error': Optional[str], 'duration_ms': int, 'source': Optional[str], 'snapshot': bool}, total=False)
PhoneHistory = TypedDict('PhoneHistory', {'object': Literal['phone.history'], 'serial': str, 'steps': List["PhoneStep"], 'runs': List[Dict[str, Any]], 'retention_days': int, 'max_steps': int}, total=False)
PhoneLiveView = TypedDict('PhoneLiveView', {'object': Literal['phone.live_view'], 'id': str, 'serial': str, 'view_only': Literal[True], 'expires_in': int, 'expires_at': str, 'viewer_url': str, 'stream_url': str}, total=False)
PhoneFleetRun = TypedDict('PhoneFleetRun', {'object': Literal['phone.fleet_run'], 'id': str, 'kind': Literal['task', 'skill'], 'goal': Optional[str], 'skill': Optional[str], 'status': Literal['running', 'completed'], 'created_at': str, 'completed_at': Optional[str], 'counts': Dict[str, Any], 'runs': List[Dict[str, Any]], 'not_started': List[Dict[str, Any]]}, total=False)
PhoneRunStarted = TypedDict('PhoneRunStarted', {'object': Literal['phone.run'], 'run_id': str, 'serial': Optional[str], 'skill': Optional[str], 'status': Literal['running']}, total=False)
PhoneToolResult = TypedDict('PhoneToolResult', {'object': Literal['phone.tool_result'], 'ok': bool, 'run_id': str, 'tool': str, 'error': str, 'message': str}, total=False)


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

    def run_agent_ag_ui_stream(self, id: str, *, messages: List[Dict[str, Any]], thread_id: str = NOT_GIVEN, run_id: str = NOT_GIVEN, parent_run_id: str = NOT_GIVEN, tools: List[Dict[str, Any]] = NOT_GIVEN, context: List[Dict[str, Any]] = NOT_GIVEN, state: Any = NOT_GIVEN, forwarded_props: Any = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Iterator[SSEEvent]:
        """Run the agent for an AG-UI frontend

        Runs one conversation turn over the AG-UI protocol (https://docs.ag-ui.com), so AG-UI
        clients such as CopilotKit's `HttpAgent` can drive the agent. The body is AG-UI's
        `RunAgentInput`. `threadId` is one of the agent's conversation ids, or any other string,
        which is kept on the conversation the first run opens so later runs continue it; no
        `threadId` opens a new conversation. The server keeps the history: the turn's input is
        the last `user` message's text. Client-side `tools`, `context` and `state` are not used.
        The answer is server-sent events, one `data:` JSON AG-UI event each: `RUN_STARTED`,
        `TEXT_MESSAGE_START` / `TEXT_MESSAGE_CONTENT` / `TEXT_MESSAGE_END`, `TOOL_CALL_START` /
        `TOOL_CALL_ARGS` / `TOOL_CALL_END` / `TOOL_CALL_RESULT` for each tool the agent uses,
        then `RUN_FINISHED` or `RUN_ERROR` (`{message, code}`). `runId` is echoed (generated when
        missing). Refusals before the stream starts are plain errors, as for messages:
        `402 spend_cap_reached` / `payment_method_required`, `409 conversation_busy`,
        `503 runtime_starting`. The reply is stored and metered like any conversation message.

        ``POST /v1/agents/{id}/ag-ui``
        """
        _body: Dict[str, Any] = {}
        if thread_id is not NOT_GIVEN:
            _body['threadId'] = thread_id
        if run_id is not NOT_GIVEN:
            _body['runId'] = run_id
        if parent_run_id is not NOT_GIVEN:
            _body['parentRunId'] = parent_run_id
        if messages is not NOT_GIVEN:
            _body['messages'] = messages
        if tools is not NOT_GIVEN:
            _body['tools'] = tools
        if context is not NOT_GIVEN:
            _body['context'] = context
        if state is not NOT_GIVEN:
            _body['state'] = state
        if forwarded_props is not NOT_GIVEN:
            _body['forwardedProps'] = forwarded_props
        return self._client.stream_request("POST", f"/v1/agents/{_q(id)}/ag-ui", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)

    def get(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Conversation":
        """Retrieve a conversation

        The conversation with its latest 200 messages, oldest first.

        ``GET /v1/conversations/{id}``
        """
        return self._client.request("GET", f"/v1/conversations/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def send_message(self, id: str, *, content: str, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "ConversationMessage":
        """Send a message

        Sends a message and answers with the agent's reply. With `stream: true` the answer is
        server-sent events: `message.delta` (`{"delta": "..."}`) while the agent writes,
        `tool.started` / `tool.completed` / `tool.failed` (`{"tool_call_id", "name", "input"?,
        "output"?, "error"?}`; arguments and results are cut at 8,000 characters) as it uses
        tools, then `message.completed` (the stored reply) or `error`. Clients should skip
        event names they don't know. The first message starts the
        project's hosted runtime: `503 runtime_starting` means retry in a few seconds.
        One message at a time per conversation (`409 conversation_busy`). A project at its
        monthly spend cap gets `402 spend_cap_reached`; one with no card on file gets
        `402 payment_method_required`. Each reply records `tokens_in` and
        `tokens_out` usage (provider list price + 15%; no token charge on the project's own key).

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

    def get_project_tracing(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "ProjectTracing":
        """The project's trace export settings

        Where the project's spans go. Header values are never returned, only their names. `404 tracing_not_configured` when none is set. Requires the `usage` scope.

        ``GET /v1/projects/{id}/tracing``
        """
        return self._client.request("GET", f"/v1/projects/{_q(id)}/tracing", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def put_project_tracing(self, id: str, *, endpoint: str, headers: Dict[str, Any] = NOT_GIVEN, service_name: str = NOT_GIVEN, enabled: bool = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "ProjectTracing":
        """Set the project's trace export

        Send a span for every agent turn, phone run and call of this project to an OTLP/HTTP collector (Langfuse, Arize Phoenix, any OpenTelemetry collector), as OTLP protobuf following the OpenTelemetry GenAI conventions. `endpoint` is the collector's OTLP base URL (`/v1/traces` is appended) or its full traces URL; it must be a public https URL. `headers` are encrypted at rest; leave them out to keep the stored ones. Export runs in the background and never fails or slows a request. Not available to account-bound keys.

        ``PUT /v1/projects/{id}/tracing``
        """
        _body: Dict[str, Any] = {}
        if endpoint is not NOT_GIVEN:
            _body['endpoint'] = endpoint
        if headers is not NOT_GIVEN:
            _body['headers'] = headers
        if service_name is not NOT_GIVEN:
            _body['service_name'] = service_name
        if enabled is not NOT_GIVEN:
            _body['enabled'] = enabled
        return self._client.request("PUT", f"/v1/projects/{_q(id)}/tracing", body=_body, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def delete_project_tracing(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Turn off and forget the project's trace export

        ``DELETE /v1/projects/{id}/tracing``
        """
        return self._client.request("DELETE", f"/v1/projects/{_q(id)}/tracing", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def test_project_tracing(self, id: str, *, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Send one test span

        Sends one `allternit.tracing.test` span to the collector now and reports its answer.

        ``POST /v1/projects/{id}/tracing/test``
        """
        return self._client.request("POST", f"/v1/projects/{_q(id)}/tracing/test", idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get_spend_cap(self, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "SpendCap":
        """The project's monthly spend cap and spend so far

        Spend is this UTC calendar month's usage at list prices (before plan credits or discounts). Requires the `usage` scope.

        ``GET /v1/projects/current/spend_cap``
        """
        return self._client.request("GET", f"/v1/projects/current/spend_cap", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def put_spend_cap(self, *, spend_cap_cents: int, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "SpendCap":
        """Lower the monthly spend cap

        An API key can only lower the cap (403 `spend_cap_raise_in_console` otherwise); raise it in the console. Requires the `usage` scope and a key not bound to an account.

        ``PUT /v1/projects/current/spend_cap``
        """
        _body: Dict[str, Any] = {}
        if spend_cap_cents is not NOT_GIVEN:
            _body['spend_cap_cents'] = spend_cap_cents
        return self._client.request("PUT", f"/v1/projects/current/spend_cap", body=_body, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

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

    def update(self, id: str, *, agent_id: Optional[str] = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "PhoneNumber":
        """Bind the agent that answers calls (agent_id, null unbinds)

        ``PATCH /v1/numbers/{id}``
        """
        _body: Dict[str, Any] = {}
        if agent_id is not NOT_GIVEN:
            _body['agent_id'] = agent_id
        return self._client.request("PATCH", f"/v1/numbers/{_q(id)}", body=_body, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

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

    def create(self, *, url: str, events: List[Literal['*', 'approval.requested', 'message.received', 'message.status', 'registration.updated', 'inbox.item.created', 'call.started', 'call.ended', 'call.transcript.ready', 'usage.threshold', 'phone.step', 'phone.run.completed', 'phone.fleet_run.completed', 'mail.received']], description: str = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
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


class VoiceResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def list_calls(self, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """List calls (filters account_id, agent_id, status; cursor pagination)

        ``GET /v1/calls``
        """
        return self._client.request("GET", f"/v1/calls", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def create_call(self, *, agent_id: str, from_number_id: str, to: str, purpose: str, record: bool = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Call":
        """Place an outbound call (consent, STOP, business hours, spend cap checked)

        ``POST /v1/calls``
        """
        _body: Dict[str, Any] = {}
        if agent_id is not NOT_GIVEN:
            _body['agent_id'] = agent_id
        if from_number_id is not NOT_GIVEN:
            _body['from_number_id'] = from_number_id
        if to is not NOT_GIVEN:
            _body['to'] = to
        if purpose is not NOT_GIVEN:
            _body['purpose'] = purpose
        if record is not NOT_GIVEN:
            _body['record'] = record
        return self._client.request("POST", f"/v1/calls", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get_call(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Call":
        """Retrieve a call

        ``GET /v1/calls/{id}``
        """
        return self._client.request("GET", f"/v1/calls/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def end_call(self, id: str, *, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Call":
        """End a call

        ``POST /v1/calls/{id}/end``
        """
        return self._client.request("POST", f"/v1/calls/{_q(id)}/end", idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def transfer_call(self, id: str, *, to: str = NOT_GIVEN, mode: Literal['warm', 'cold'] = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Call":
        """Transfer to one of the agent's transfer_targets

        ``POST /v1/calls/{id}/transfer``
        """
        _body: Dict[str, Any] = {}
        if to is not NOT_GIVEN:
            _body['to'] = to
        if mode is not NOT_GIVEN:
            _body['mode'] = mode
        return self._client.request("POST", f"/v1/calls/{_q(id)}/transfer", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get_call_transcript(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Call transcript

        ``GET /v1/calls/{id}/transcript``
        """
        return self._client.request("GET", f"/v1/calls/{_q(id)}/transcript", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get_call_recording(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Recording URL (only when the call was recorded)

        ``GET /v1/calls/{id}/recording``
        """
        return self._client.request("GET", f"/v1/calls/{_q(id)}/recording", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def simulate_call_turn(self, id: str, *, text: str, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Sandbox: play what the caller says

        ``POST /v1/calls/{id}/simulate_turn``
        """
        _body: Dict[str, Any] = {}
        if text is not NOT_GIVEN:
            _body['text'] = text
        return self._client.request("POST", f"/v1/calls/{_q(id)}/simulate_turn", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def simulate_inbound_call(self, id: str, *, from_: str, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Call":
        """Sandbox: play an inbound call

        ``POST /v1/numbers/{id}/simulate_call``
        """
        _body: Dict[str, Any] = {}
        if from_ is not NOT_GIVEN:
            _body['from'] = from_
        return self._client.request("POST", f"/v1/numbers/{_q(id)}/simulate_call", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]


class RealtimeResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def create_session(self, *, agent_id: str, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Ephemeral client token for talking to an agent (60 s to start)

        ``POST /v1/realtime/sessions``
        """
        _body: Dict[str, Any] = {}
        if agent_id is not NOT_GIVEN:
            _body['agent_id'] = agent_id
        return self._client.request("POST", f"/v1/realtime/sessions", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]


class ChannelsResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def list(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """List an account's channels

        Channel connections of the account (`?agent_id=` filter). Scope `channels`.

        ``GET /v1/accounts/{id}/channels``
        """
        return self._client.request("GET", f"/v1/accounts/{_q(id)}/channels", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def connect(self, id: str, kind: str, *, body: Optional[Dict[str, Any]] = None, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Connect a channel for an agent

        `{agent_id, return_url?, local_part?}`. `email` connects at once; `slack` answers `pending` with `connect_url`; kinds without a production app answer 400 `channel_kind_unavailable`.

        ``POST /v1/accounts/{id}/channels/{kind}/connect``
        """
        _body: Dict[str, Any] = dict(body or {})
        return self._client.request("POST", f"/v1/accounts/{_q(id)}/channels/{_q(kind)}/connect", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get(self, id: str, channel_id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Retrieve a channel

        One channel connection.

        ``GET /v1/accounts/{id}/channels/{channel_id}``
        """
        return self._client.request("GET", f"/v1/accounts/{_q(id)}/channels/{_q(channel_id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def delete(self, id: str, channel_id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Disconnect a channel

        The runtime releases the channel, then the record is deleted.

        ``DELETE /v1/accounts/{id}/channels/{channel_id}``
        """
        return self._client.request("DELETE", f"/v1/accounts/{_q(id)}/channels/{_q(channel_id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]


class TwinResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def list_people(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """List people

        People the account's agents talk to (cursor `limit`/`after`). Scope `twin`.

        ``GET /v1/accounts/{id}/people``
        """
        return self._client.request("GET", f"/v1/accounts/{_q(id)}/people", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get_person(self, id: str, person_id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Retrieve a person

        With identities and the account's facts.

        ``GET /v1/accounts/{id}/people/{person_id}``
        """
        return self._client.request("GET", f"/v1/accounts/{_q(id)}/people/{_q(person_id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def update_person(self, id: str, person_id: str, *, body: Optional[Dict[str, Any]] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Update a person

        `{name?, notes?, org?}`. 409 `person_shared` when another account's agents also know them.

        ``PATCH /v1/accounts/{id}/people/{person_id}``
        """
        _body: Dict[str, Any] = dict(body or {})
        return self._client.request("PATCH", f"/v1/accounts/{_q(id)}/people/{_q(person_id)}", body=_body, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_inbox(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """List inbox items

        `?status=open|resolved|all`, newest first.

        ``GET /v1/accounts/{id}/inbox``
        """
        return self._client.request("GET", f"/v1/accounts/{_q(id)}/inbox", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def resolve_inbox_item(self, id: str, item_id: str, *, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Resolve an inbox item

        Marks it resolved.

        ``POST /v1/accounts/{id}/inbox/{item_id}/resolve``
        """
        return self._client.request("POST", f"/v1/accounts/{_q(id)}/inbox/{_q(item_id)}/resolve", idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_approvals(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """List approvals

        `?status=pending|approved|denied|all`, newest first.

        ``GET /v1/accounts/{id}/approvals``
        """
        return self._client.request("GET", f"/v1/accounts/{_q(id)}/approvals", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def approve_approval(self, id: str, approval_id: str, *, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Approve a held action

        The API key is recorded as the actor.

        ``POST /v1/accounts/{id}/approvals/{approval_id}/approve``
        """
        return self._client.request("POST", f"/v1/accounts/{_q(id)}/approvals/{_q(approval_id)}/approve", idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def deny_approval(self, id: str, approval_id: str, *, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Deny a held action

        The API key is recorded as the actor.

        ``POST /v1/accounts/{id}/approvals/{approval_id}/deny``
        """
        return self._client.request("POST", f"/v1/accounts/{_q(id)}/approvals/{_q(approval_id)}/deny", idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_memory(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """List account memory

        Facts every agent of the account knows.

        ``GET /v1/accounts/{id}/memory``
        """
        return self._client.request("GET", f"/v1/accounts/{_q(id)}/memory", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def add_memory(self, id: str, *, body: Optional[Dict[str, Any]] = None, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Add account memory

        `{content, subject?, kind?, source_ref?}`; at most 500 per account.

        ``POST /v1/accounts/{id}/memory``
        """
        _body: Dict[str, Any] = dict(body or {})
        return self._client.request("POST", f"/v1/accounts/{_q(id)}/memory", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def delete_memory(self, id: str, memory_id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Delete account memory

        Removes one item; agents get the change on their next sync.

        ``DELETE /v1/accounts/{id}/memory/{memory_id}``
        """
        return self._client.request("DELETE", f"/v1/accounts/{_q(id)}/memory/{_q(memory_id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get_autonomy(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Retrieve an agent's autonomy

        Agent-wide `level` and per channel / person `rules`.

        ``GET /v1/agents/{id}/autonomy``
        """
        return self._client.request("GET", f"/v1/agents/{_q(id)}/autonomy", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def put_autonomy(self, id: str, *, body: Optional[Dict[str, Any]] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Replace an agent's autonomy

        `{level?, rules:[{channel?, person?, level, limits?}]}` replaces every rule.

        ``PUT /v1/agents/{id}/autonomy``
        """
        _body: Dict[str, Any] = dict(body or {})
        return self._client.request("PUT", f"/v1/agents/{_q(id)}/autonomy", body=_body, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]


class ComputersResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def list(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "ComputerList":
        """List computers

        Hosted-driver computers in this project (an account-bound key sees its account's only). Requires the `computers` scope and the project's hosted driver flag (404 `hosted_driver_disabled` until Allternit turns it on).

        ``GET /v1/computers``
        """
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        return self._client.request("GET", f"/v1/computers", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_all(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Iterator["Computer"]:
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
            return self._client.request("GET", f"/v1/computers", query=q, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]
        return self._client.paginate(_fetch, after if after is not NOT_GIVEN else None)

    def create(self, *, name: str = NOT_GIVEN, account_id: str = NOT_GIVEN, metadata: Dict[str, Any] = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Computer":
        """Create a computer

        Provisions a cloud computer your agents drive with the computer and browser toolsets. 402 `payment_method_required` without a card on file, 402 `spend_cap_reached` at the spend cap; 429 `concurrency_limit` when this key already has `per_key_concurrency` live computers.

        ``POST /v1/computers``
        """
        _body: Dict[str, Any] = {}
        if name is not NOT_GIVEN:
            _body['name'] = name
        if account_id is not NOT_GIVEN:
            _body['account_id'] = account_id
        if metadata is not NOT_GIVEN:
            _body['metadata'] = metadata
        return self._client.request("POST", f"/v1/computers", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Computer":
        """Get a computer

        ``GET /v1/computers/{id}``
        """
        return self._client.request("GET", f"/v1/computers/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def delete(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Computer":
        """Delete a computer

        Deletes the computer and its disk. Running minutes up to now are billed.

        ``DELETE /v1/computers/{id}``
        """
        return self._client.request("DELETE", f"/v1/computers/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def start(self, id: str, *, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Computer":
        """Start a computer

        ``POST /v1/computers/{id}/start``
        """
        return self._client.request("POST", f"/v1/computers/{_q(id)}/start", idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def stop(self, id: str, *, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Computer":
        """Stop a computer

        ``POST /v1/computers/{id}/stop``
        """
        return self._client.request("POST", f"/v1/computers/{_q(id)}/stop", idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def call_toolset(self, id: str, *, toolset: Literal['computer', 'browser'], member: str, input: Dict[str, Any] = NOT_GIVEN, run_id: str = NOT_GIVEN, turn_id: str = NOT_GIVEN, call_index: int = NOT_GIVEN, model_frame: Dict[str, Any] = NOT_GIVEN, coordinate_space: Literal['pixels', 'normalized_1000'] = NOT_GIVEN, approval_grant: str = NOT_GIVEN, browser_session_id: str = NOT_GIVEN, enable: List[str] = NOT_GIVEN, agent_id: str = NOT_GIVEN, agent_name: str = NOT_GIVEN, step: str = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "ToolsetResult":
        """Run one toolset call

        One `allternit.computer.v1` / `allternit.browser.v1` call (member names and inputs match Anthropic's `computer_toolset_20260801` / `browser_toolset_20260801`). Runs through the computer's executor: contract validation, control lease, policy, approval, audit, then the action. Action failures are 200 with `is_error: true`.
        409 `approval_required` carries `approval`: approve it (`POST /v1/computers/{id}/approvals/{approval_id}`, or the project owner in the console), then resend the same call with `approval_grant`.

        ``POST /v1/computers/{id}/toolset``
        """
        _body: Dict[str, Any] = {}
        if toolset is not NOT_GIVEN:
            _body['toolset'] = toolset
        if member is not NOT_GIVEN:
            _body['member'] = member
        if input is not NOT_GIVEN:
            _body['input'] = input
        if run_id is not NOT_GIVEN:
            _body['run_id'] = run_id
        if turn_id is not NOT_GIVEN:
            _body['turn_id'] = turn_id
        if call_index is not NOT_GIVEN:
            _body['call_index'] = call_index
        if model_frame is not NOT_GIVEN:
            _body['model_frame'] = model_frame
        if coordinate_space is not NOT_GIVEN:
            _body['coordinate_space'] = coordinate_space
        if approval_grant is not NOT_GIVEN:
            _body['approval_grant'] = approval_grant
        if browser_session_id is not NOT_GIVEN:
            _body['browser_session_id'] = browser_session_id
        if enable is not NOT_GIVEN:
            _body['enable'] = enable
        if agent_id is not NOT_GIVEN:
            _body['agent_id'] = agent_id
        if agent_name is not NOT_GIVEN:
            _body['agent_name'] = agent_name
        if step is not NOT_GIVEN:
            _body['step'] = step
        return self._client.request("POST", f"/v1/computers/{_q(id)}/toolset", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get_toolset_schema(self, id: str, *, toolset: Literal['computer', 'browser'] = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Members available on this computer

        ``GET /v1/computers/{id}/toolset/schema``
        """
        _query: Dict[str, Any] = {}
        if toolset is not NOT_GIVEN:
            _query['toolset'] = toolset
        return self._client.request("GET", f"/v1/computers/{_q(id)}/toolset/schema", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_events(self, id: str, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "ComputerEventList":
        """Poll the computer's action events

        `computer.action` events, oldest first. Pass the last page's `next_cursor` as `after` to poll.

        ``GET /v1/computers/{id}/events``
        """
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        return self._client.request("GET", f"/v1/computers/{_q(id)}/events", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_events_all(self, id: str, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Iterator["ComputerEvent"]:
        """Every item of ``list_events``, fetching pages as you iterate."""
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        def _fetch(cursor: Optional[str]) -> Page:
            q = dict(_query)
            if cursor is not None:
                q['after'] = cursor
            return self._client.request("GET", f"/v1/computers/{_q(id)}/events", query=q, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]
        return self._client.paginate(_fetch, after if after is not NOT_GIVEN else None)

    def approve_action(self, id: str, approval_id: str, *, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Approve a held action

        The project owner (console) can always approve. An API key can only when the project's `approval_mode` is `api_key` (403 `approval_requires_owner` otherwise).

        ``POST /v1/computers/{id}/approvals/{approval_id}``
        """
        return self._client.request("POST", f"/v1/computers/{_q(id)}/approvals/{_q(approval_id)}", idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get_settings(self, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "ComputerSettings":
        """The project's hosted driver settings

        Readable while the flag is off (so you can see it). Requires the `computers` scope.

        ``GET /v1/computer_settings``
        """
        return self._client.request("GET", f"/v1/computer_settings", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def update_settings(self, *, approval_mode: Literal['owner', 'api_key'] = NOT_GIVEN, per_key_concurrency: int = NOT_GIVEN, browser_toolset: bool = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "ComputerSettings":
        """Change the hosted driver settings

        Console only (403 `settings_require_console` for API keys), so a key can't widen its own approvals. `hosted_driver_enabled` is set by Allternit.

        ``PATCH /v1/computer_settings``
        """
        _body: Dict[str, Any] = {}
        if approval_mode is not NOT_GIVEN:
            _body['approval_mode'] = approval_mode
        if per_key_concurrency is not NOT_GIVEN:
            _body['per_key_concurrency'] = per_key_concurrency
        if browser_toolset is not NOT_GIVEN:
            _body['browser_toolset'] = browser_toolset
        return self._client.request("PATCH", f"/v1/computer_settings", body=_body, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]


class PhonesResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def list(self, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "PhoneList":
        """List the owner's phones

        The project owner's own phones, linked to their Allternit Desktop (Phone Link, adb or a USB Agent). Requires the `phones` scope and a key that is not bound to an account (403 `account_bound_key`).
        While the Desktop is offline the phones the cloud last saw are listed with `online: false` and `desktop_online: false`. Not paginated (a person links a handful).

        ``GET /v1/phones``
        """
        return self._client.request("GET", f"/v1/phones", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_skills(self, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "PhoneSkillList":
        """List saved phone skills

        Recorded phone runs that replay with no model. Each lists the `inputs` a run must pass (typed text is never stored). 503 `phone_desktop_offline` while the Desktop is offline.

        ``GET /v1/phones/skills``
        """
        return self._client.request("GET", f"/v1/phones/skills", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def run_skill(self, name: str, *, serial: str = NOT_GIVEN, inputs: Dict[str, Any] = NOT_GIVEN, bot_id: str = NOT_GIVEN, brain: str = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "PhoneRunStarted":
        """Replay a phone skill

        Starts the replay in the background and answers 202 with its `run_id`. Each step is a `phone.step` webhook; the end is `phone.run.completed`. A step whose element moved re-decides once on `bot_id`'s brain (or `brain`). Skills never send texts or place calls.

        ``POST /v1/phones/skills/{name}/run``
        """
        _body: Dict[str, Any] = {}
        if serial is not NOT_GIVEN:
            _body['serial'] = serial
        if inputs is not NOT_GIVEN:
            _body['inputs'] = inputs
        if bot_id is not NOT_GIVEN:
            _body['bot_id'] = bot_id
        if brain is not NOT_GIVEN:
            _body['brain'] = brain
        return self._client.request("POST", f"/v1/phones/skills/{_q(name)}/run", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get_history(self, serial: str, *, run_id: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "PhoneHistory":
        """A phone's run history

        Every phone tool call on this phone (500 steps / 30 days), newest run first. Typed text, numbers and message bodies are redacted on the Desktop; frames and screen text stay there.

        ``GET /v1/phones/{serial}/history``
        """
        _query: Dict[str, Any] = {}
        if run_id is not NOT_GIVEN:
            _query['run_id'] = run_id
        return self._client.request("GET", f"/v1/phones/{_q(serial)}/history", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def call_tool(self, serial: str, *, name: str, arguments: Dict[str, Any] = NOT_GIVEN, run_id: str = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "PhoneToolResult":
        """Run one phone tool

        One phone tool through the Desktop phone gateway, exactly as a bot calls it: `phone.devices`, `phone.screenshot`, `phone.read_ui`, `phone.screen_text`, `phone.tap`, `phone.type`, `phone.find_and_tap`, `phone.swipe`, `phone.home`, `phone.back`, `phone.key`, `phone.open_app`, `phone.sms.read`, `phone.sms.send`, `phone.call.dial`. An iPhone running the Allternit app (serial `app:<phoneId>`) is screen only plus its in-app browser: `phone.screenshot`, `phone.devices` and `phone.web.open`, `phone.web.snapshot`, `phone.web.click`, `phone.web.type`, `phone.web.scroll`, `phone.web.back`, `phone.web.wait` (the app must be open; taps on native apps answer `not_supported`).
        `phone.sms.send` and `phone.call.dial` wait for the person to approve on their Desktop (up to 80 s; then 503 `phone_timeout`); a refusal is a 200 result with `ok: false, error: approval_denied`. Tool failures are 200 with `ok: false`. 409 `phone_offline` when the phone isn't connected, 503 `phone_desktop_offline` / `phone_gateway_unavailable` when the Desktop or its phone gateway isn't running. The call is a `phone.step` webhook like any other.

        ``POST /v1/phones/{serial}/tool``
        """
        _body: Dict[str, Any] = {}
        if name is not NOT_GIVEN:
            _body['name'] = name
        if arguments is not NOT_GIVEN:
            _body['arguments'] = arguments
        if run_id is not NOT_GIVEN:
            _body['run_id'] = run_id
        return self._client.request("POST", f"/v1/phones/{_q(serial)}/tool", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def run_task(self, serial: str, *, goal: str, max_steps: int = NOT_GIVEN, bot_id: str = NOT_GIVEN, brain: str = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "PhoneRunStarted":
        """Give a phone a goal

        The phone task loop on the Desktop (read the screen, one action, repeat) with `bot_id`'s brain, `brain`, or the default. Answers 202 with the `run_id`; each step is a `phone.step` webhook and the end is `phone.run.completed`. Tasks never send texts or place calls. At most 5 minutes.

        ``POST /v1/phones/{serial}/task``
        """
        _body: Dict[str, Any] = {}
        if goal is not NOT_GIVEN:
            _body['goal'] = goal
        if max_steps is not NOT_GIVEN:
            _body['max_steps'] = max_steps
        if bot_id is not NOT_GIVEN:
            _body['bot_id'] = bot_id
        if brain is not NOT_GIVEN:
            _body['brain'] = brain
        return self._client.request("POST", f"/v1/phones/{_q(serial)}/task", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get_live_view(self, serial: str, *, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "PhoneLiveView":
        """Watch a phone live

        A view-only live view of the phone's screen for 10 minutes: `viewer_url` is a page you can open or put in an iframe, and `stream_url` is the raw stream (`{type:"meta"}` JSON, then H.264 Annex-B frames) for your own player. Both URLs carry the live view id as their only credential and stop working after `expires_in` seconds; neither can send input. A new view needs a new call. 409 `phone_offline` when the phone isn't connected to the Desktop.

        ``POST /v1/phones/{serial}/live``
        """
        return self._client.request("POST", f"/v1/phones/{_q(serial)}/live", idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def start_fleet_run(self, *, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Run one task or skill on many phones

        Starts one background run per listed phone: a task (`goal`) or a saved skill (`skill`). Answers 202 with the fleet run; phones whose run could not start are in `not_started` with the error. Every run counts against the plan's concurrent phone-run limit (Sandbox 1, Pay as you go 5, Growth 25, Enterprise by contract); asking for more than the room left is 429 `phone_run_limit` and starts nothing. Each run ends with `phone.run.completed`; the whole fleet run with `phone.fleet_run.completed`.

        ``POST /v1/phones/runs``
        """
        return self._client.request("POST", f"/v1/phones/runs", idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get_fleet_run(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """A fleet run

        The fleet run and each phone's run (status, ok, start and end).

        ``GET /v1/phones/runs/{id}``
        """
        return self._client.request("GET", f"/v1/phones/runs/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]


class MailboxesResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def list(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, account_id: str = NOT_GIVEN, agent_id: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "MailboxList":
        """List mailboxes

        ``GET /v1/mailboxes``
        """
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        if account_id is not NOT_GIVEN:
            _query['account_id'] = account_id
        if agent_id is not NOT_GIVEN:
            _query['agent_id'] = agent_id
        return self._client.request("GET", f"/v1/mailboxes", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_all(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, account_id: str = NOT_GIVEN, agent_id: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Iterator["Mailbox"]:
        """Every item of ``list``, fetching pages as you iterate."""
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        if account_id is not NOT_GIVEN:
            _query['account_id'] = account_id
        if agent_id is not NOT_GIVEN:
            _query['agent_id'] = agent_id
        def _fetch(cursor: Optional[str]) -> Page:
            q = dict(_query)
            if cursor is not None:
                q['after'] = cursor
            return self._client.request("GET", f"/v1/mailboxes", query=q, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]
        return self._client.paginate(_fetch, after if after is not NOT_GIVEN else None)

    def create_mailbox(self, *, account_id: str = NOT_GIVEN, agent_id: str = NOT_GIVEN, local_part: str = NOT_GIVEN, display_name: str = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Mailbox":
        """Create a mailbox for an account or agent

        An address on Allternit Mail (`local_part@bots.allternit.com`; `-2`, `-3`… when taken). With `agent_id`, the account is the agent's. Needs a card on file; live projects are billed `mailbox_month` ($2 a month). Requires the `mail` scope.

        ``POST /v1/mailboxes``
        """
        _body: Dict[str, Any] = {}
        if account_id is not NOT_GIVEN:
            _body['account_id'] = account_id
        if agent_id is not NOT_GIVEN:
            _body['agent_id'] = agent_id
        if local_part is not NOT_GIVEN:
            _body['local_part'] = local_part
        if display_name is not NOT_GIVEN:
            _body['display_name'] = display_name
        return self._client.request("POST", f"/v1/mailboxes", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get_mailbox(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Mailbox":
        """Retrieve a mailbox

        ``GET /v1/mailboxes/{id}``
        """
        return self._client.request("GET", f"/v1/mailboxes/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def delete_mailbox(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Delete a mailbox

        Removes the address. A mailbox inside an identity answers `409 mailbox_in_identity`; delete the identity instead.

        ``DELETE /v1/mailboxes/{id}``
        """
        return self._client.request("DELETE", f"/v1/mailboxes/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_mail_messages(self, id: str, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, verification: bool = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "MailMessageList":
        """List a mailbox's messages

        Newest first. Each carries the one-time codes and sign-in links found in it.

        ``GET /v1/mailboxes/{id}/messages``
        """
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        if verification is not NOT_GIVEN:
            _query['verification'] = verification
        return self._client.request("GET", f"/v1/mailboxes/{_q(id)}/messages", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_mail_messages_all(self, id: str, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, verification: bool = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Iterator["MailMessage"]:
        """Every item of ``list_mail_messages``, fetching pages as you iterate."""
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        if verification is not NOT_GIVEN:
            _query['verification'] = verification
        def _fetch(cursor: Optional[str]) -> Page:
            q = dict(_query)
            if cursor is not None:
                q['after'] = cursor
            return self._client.request("GET", f"/v1/mailboxes/{_q(id)}/messages", query=q, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]
        return self._client.paginate(_fetch, after if after is not NOT_GIVEN else None)

    def get_mail_message(self, id: str, mid: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Read one message

        ``GET /v1/mailboxes/{id}/messages/{mid}``
        """
        return self._client.request("GET", f"/v1/mailboxes/{_q(id)}/messages/{_q(mid)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def read_mail_otp(self, id: str, *, wait_seconds: int = NOT_GIVEN, since: str = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Wait for the newest one-time code

        The newest message with a code or sign-in link received after `since` (default the last 10 minutes), waiting up to `wait_seconds` for one to arrive. `found` is false when none came.

        ``POST /v1/mailboxes/{id}/otp``
        """
        _body: Dict[str, Any] = {}
        if wait_seconds is not NOT_GIVEN:
            _body['wait_seconds'] = wait_seconds
        if since is not NOT_GIVEN:
            _body['since'] = since
        return self._client.request("POST", f"/v1/mailboxes/{_q(id)}/otp", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def send_mail(self, id: str, *, serials: List[str], goal: str = NOT_GIVEN, skill: str = NOT_GIVEN, inputs: Dict[str, Any] = NOT_GIVEN, max_steps: int = NOT_GIVEN, bot_id: str = NOT_GIVEN, brain: str = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "PhoneFleetRun":
        """Send an email from a mailbox

        Live projects only. The call is the approval, so the send is queued at once; `pending_approval` only when Allternit Mail held it and could not release it.

        ``POST /v1/mailboxes/{id}/send``
        """
        _body: Dict[str, Any] = {}
        if serials is not NOT_GIVEN:
            _body['serials'] = serials
        if goal is not NOT_GIVEN:
            _body['goal'] = goal
        if skill is not NOT_GIVEN:
            _body['skill'] = skill
        if inputs is not NOT_GIVEN:
            _body['inputs'] = inputs
        if max_steps is not NOT_GIVEN:
            _body['max_steps'] = max_steps
        if bot_id is not NOT_GIVEN:
            _body['bot_id'] = bot_id
        if brain is not NOT_GIVEN:
            _body['brain'] = brain
        return self._client.request("POST", f"/v1/mailboxes/{_q(id)}/send", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]


class IdentitiesResource:
    def __init__(self, client: Transport) -> None:
        self._client = client

    def list(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, account_id: str = NOT_GIVEN, agent_id: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "IdentityList":
        """List agent identities

        ``GET /v1/identities``
        """
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        if account_id is not NOT_GIVEN:
            _query['account_id'] = account_id
        if agent_id is not NOT_GIVEN:
            _query['agent_id'] = agent_id
        return self._client.request("GET", f"/v1/identities", query=_query, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def list_all(self, *, limit: int = NOT_GIVEN, after: str = NOT_GIVEN, account_id: str = NOT_GIVEN, agent_id: str = NOT_GIVEN, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Iterator["Identity"]:
        """Every item of ``list``, fetching pages as you iterate."""
        _query: Dict[str, Any] = {}
        if limit is not NOT_GIVEN:
            _query['limit'] = limit
        if after is not NOT_GIVEN:
            _query['after'] = after
        if account_id is not NOT_GIVEN:
            _query['account_id'] = account_id
        if agent_id is not NOT_GIVEN:
            _query['agent_id'] = agent_id
        def _fetch(cursor: Optional[str]) -> Page:
            q = dict(_query)
            if cursor is not None:
                q['after'] = cursor
            return self._client.request("GET", f"/v1/identities", query=q, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]
        return self._client.paginate(_fetch, after if after is not NOT_GIVEN else None)

    def create_identity(self, *, account_id: str = NOT_GIVEN, agent_id: str = NOT_GIVEN, area_code: str = NOT_GIVEN, number_type: Literal['local', 'toll_free'] = NOT_GIVEN, phone_serial: str = NOT_GIVEN, local_part: str = NOT_GIVEN, display_name: str = NOT_GIVEN, idempotency_key: Optional[str] = None, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Identity":
        """Give an agent a number, a mailbox and a phone in one call

        Gets a number (simulated in sandbox; in live the first available, in `area_code` when given), a mailbox and, with `phone_serial`, binds one of the owner's linked phones. Requires the `numbers` and `mail` scopes, plus `phones` (and an unbound key) with `phone_serial`. Billed as the Agent identity bundle ($9 a month, meter `identity_month`).

        ``POST /v1/identities``
        """
        _body: Dict[str, Any] = {}
        if account_id is not NOT_GIVEN:
            _body['account_id'] = account_id
        if agent_id is not NOT_GIVEN:
            _body['agent_id'] = agent_id
        if area_code is not NOT_GIVEN:
            _body['area_code'] = area_code
        if number_type is not NOT_GIVEN:
            _body['number_type'] = number_type
        if phone_serial is not NOT_GIVEN:
            _body['phone_serial'] = phone_serial
        if local_part is not NOT_GIVEN:
            _body['local_part'] = local_part
        if display_name is not NOT_GIVEN:
            _body['display_name'] = display_name
        return self._client.request("POST", f"/v1/identities", body=_body, idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def get_identity(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> "Identity":
        """Retrieve an identity

        ``GET /v1/identities/{id}``
        """
        return self._client.request("GET", f"/v1/identities/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]

    def delete_identity(self, id: str, *, timeout: Optional[float] = None, extra_headers: Optional[Dict[str, str]] = None) -> Dict[str, Any]:
        """Delete an identity

        Releases the number and removes the mailbox.

        ``DELETE /v1/identities/{id}``
        """
        return self._client.request("DELETE", f"/v1/identities/{_q(id)}", timeout=timeout, extra_headers=extra_headers)  # type: ignore[return-value]


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
        self.voice = VoiceResource(client)
        self.realtime = RealtimeResource(client)
        self.channels = ChannelsResource(client)
        self.twin = TwinResource(client)
        self.computers = ComputersResource(client)
        self.phones = PhonesResource(client)
        self.mailboxes = MailboxesResource(client)
        self.identities = IdentitiesResource(client)
