from __future__ import annotations

import time
from typing import Any, Callable, Dict, Iterator, Optional
from urllib.parse import quote

import httpx

from .errors import HumanIntentRequiredError, to_http_error


def _snake(d: Dict[str, Any]) -> Dict[str, Any]:
    return {k: v for k, v in d.items() if v is not None}


class AllternitAgents:
    """Sync client for the AAI REST surface. Pass `transport=httpx.MockTransport(...)` in tests."""

    def __init__(self, base_url: str, token: Optional[str] = None, api_prefix: str = "/api/v1",
                 transport: Optional[httpx.BaseTransport] = None, timeout: float = 30.0):
        headers = {"accept": "application/json"}
        if token:
            headers["authorization"] = f"Bearer {token}"
        self._http = httpx.Client(base_url=base_url.rstrip("/") + api_prefix, headers=headers, transport=transport, timeout=timeout)

    def close(self) -> None:
        self._http.close()

    def request(self, method: str, path: str, body: Any = None, params: Optional[Dict[str, Any]] = None) -> Any:
        params = {k: v for k, v in (params or {}).items() if v is not None}
        res = self._http.request(method, path, json=body, params=params or None)
        try:
            data = res.json() if res.content else None
        except ValueError:
            data = {"error": res.text}
        if res.status_code >= 400:
            raise to_http_error(res.status_code, data)
        return data

    @staticmethod
    def _q(s: str) -> str:
        return quote(s, safe="")

    # accounts
    def create_account(self, vendor: str, auth_type: str, **fields: Any) -> Any:
        return self.request("POST", "/gateway/provider-accounts", _snake({"vendor": vendor, "auth_type": auth_type, **fields}))

    def list_accounts(self, vendor: Optional[str] = None, state: Optional[str] = None) -> Any:
        return self.request("GET", "/gateway/provider-accounts", params={"vendor": vendor, "state": state})

    def get_account(self, account_id: str) -> Any:
        return self.request("GET", f"/gateway/provider-accounts/{self._q(account_id)}")

    def set_connection_state(self, account_id: str, state: str, reason: Optional[str] = None) -> Any:
        return self.request("PATCH", f"/gateway/provider-accounts/{self._q(account_id)}", _snake({"state": state, "reason": reason}))

    def set_secret(self, account_id: str, api_key: str) -> Any:
        return self.request("POST", f"/gateway/provider-accounts/{self._q(account_id)}/secret", {"api_key": api_key})

    def clear_secret(self, account_id: str) -> Any:
        return self.request("DELETE", f"/gateway/provider-accounts/{self._q(account_id)}/secret")

    def discover_agents(self, account_id: str) -> Any:
        return self.request("GET", f"/gateway/provider-accounts/{self._q(account_id)}/agents")

    # bots
    def bind_execution(self, bot_id: str, **fields: Any) -> Any:
        return self.request("PUT", f"/gateway/bots/{self._q(bot_id)}/execution-binding", _snake(fields))

    def get_binding(self, bot_id: str) -> Any:
        return self.request("GET", f"/gateway/bots/{self._q(bot_id)}/execution-binding")

    def list_bindings(self) -> Any:
        return self.request("GET", "/gateway/execution-bindings")

    # threads
    def send_turn(self, session_id: str, text: str, metadata: Optional[Dict[str, Any]] = None) -> Any:
        """Raises ApprovalRequiredError (428), ConflictError (409), RateLimitedError (429)."""
        return self.request("POST", f"/agent-sessions/{self._q(session_id)}/messages", _snake({"text": text, "metadata": metadata}))

    def sync(self, thread_id: str) -> Any:
        return self.request("POST", f"/threads/{self._q(thread_id)}/gateway/sync")

    def events(self, thread_id: str, after: Optional[int] = None, limit: Optional[int] = None) -> Any:
        return self.request("GET", f"/threads/{self._q(thread_id)}/events", params={"after": after, "limit": limit})

    def stream_events(self, thread_id: str, after: int = 0, max_idle_polls: Optional[int] = None, poll_s: float = 1.0,
                      max_backoff_s: float = 15.0, sleep: Callable[[float], None] = time.sleep) -> Iterator[Dict[str, Any]]:
        """Generator over events using the `after` cursor; exponential backoff when idle or on 5xx/429."""
        idle = failures = 0
        while True:
            try:
                r = self.events(thread_id, after=after)
                page = r if isinstance(r, list) else (r or {}).get("events", [])
                failures = 0
            except Exception as e:  # noqa: BLE001
                st = getattr(e, "status", None)
                if isinstance(e, httpx.TransportError) or (st is not None and (st >= 500 or st == 429)):
                    failures += 1
                    if failures > 8:
                        raise
                    sleep(min(max_backoff_s, poll_s * 2 ** failures))
                    continue
                raise
            if page:
                idle = 0
                for ev in page:
                    after = max(after, ev["sequence"])
                    yield ev
                continue
            idle += 1
            if max_idle_polls is not None and idle >= max_idle_polls:
                return
            sleep(min(max_backoff_s, poll_s * 2 ** min(idle - 1, 6)))

    # approvals
    def list_approvals(self, thread_id: str, state: Optional[str] = None) -> Any:
        return self.request("GET", f"/threads/{self._q(thread_id)}/approvals", params={"state": state})

    def respond_approval(self, approval_id: str, decision: str, *, human_intent: bool = False) -> Any:
        if human_intent is not True:
            raise HumanIntentRequiredError()
        return self.request("POST", f"/gateway/approvals/{self._q(approval_id)}/respond", {"decision": decision, "actor": {"type": "user"}})

    # vendor packs
    def record_gap(self, vendor: str, capability: str, surface: str, **fields: Any) -> Any:
        return self.request("POST", f"/gateway/vendor-packs/{self._q(vendor)}/gaps", _snake({"capability": capability, "surface": surface, **fields}))

    def gaps(self, vendor: str, status: Optional[str] = None) -> Any:
        return self.request("GET", f"/gateway/vendor-packs/{self._q(vendor)}/gaps", params={"status": status})

    def parity(self, vendor: str) -> Any:
        return self.request("GET", f"/gateway/vendor-packs/{self._q(vendor)}/parity")

    # channel bindings
    def bind_channel(self, thread_id: str, provider: str, external_conversation_id: str, **fields: Any) -> Any:
        return self.request("POST", f"/gateway/threads/{self._q(thread_id)}/channel-bindings",
                            _snake({"provider": provider, "external_conversation_id": external_conversation_id, **fields}))

    def list_channel_bindings(self, thread_id: str) -> Any:
        return self.request("GET", f"/gateway/threads/{self._q(thread_id)}/channel-bindings")
