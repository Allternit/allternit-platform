"""Plain client for the Allternit hosted computer API (/v1/computers). Standard library only."""

from __future__ import annotations

import json
import os
import uuid
from typing import Any, Callable, Dict, List, Optional
from urllib.error import HTTPError, URLError
from urllib.parse import quote, urlencode
from urllib.request import Request, urlopen

DEFAULT_BASE_URL = "https://api.allternit.com"
DEFAULT_TIMEOUT = 60.0

ToolsetResult = Dict[str, Any]
"""``{is_error, content:[{type:"text",text}|{type:"image",media_type,data}], browser_state?, screen?, error?}``"""


class AllternitApiError(Exception):
    def __init__(self, status: int, error: Dict[str, Any], body: Any) -> None:
        super().__init__(error.get("message") or f"Allternit API error {status}")
        self.status = status
        self.type = error.get("type", "api_error")
        self.code = error.get("code", "unknown")
        self.param = error.get("param")
        self.body = body


class ApprovalRequiredError(AllternitApiError):
    """409 ``approval_required``: the call is held until someone approves it."""

    def __init__(self, error: Dict[str, Any], approval: Dict[str, Any], result: ToolsetResult, body: Any) -> None:
        super().__init__(409, error, body)
        self.approval = approval
        self.result = result


class ComputerBusyError(AllternitApiError):
    """423 ``computer_busy`` / ``computer_controlled_elsewhere``: someone else holds the computer right now."""


class SandboxRequiredError(AllternitApiError):
    """409 ``sandbox_required``: the call only runs on a sandbox (cloud/bot) computer."""


class ComputerConflictError(AllternitApiError):
    """409 ``computer_conflict``: the call conflicts with another subtask or lease on the computer."""


def _typed_api_error(status: int, err: Dict[str, Any], body: Any) -> AllternitApiError:
    code = err.get("code")
    if code in ("computer_busy", "computer_controlled_elsewhere"):
        return ComputerBusyError(status, err, body)
    if code == "sandbox_required":
        return SandboxRequiredError(status, err, body)
    if code == "computer_conflict":
        return ComputerConflictError(status, err, body)
    return AllternitApiError(status, err, body)


class AllternitComputers:
    def __init__(self, api_key: Optional[str] = None, base_url: Optional[str] = None, timeout: float = DEFAULT_TIMEOUT) -> None:
        key = api_key or os.environ.get("ALLTERNIT_API_KEY")
        if not key:
            raise ValueError("AllternitComputers: api_key is required (or set ALLTERNIT_API_KEY).")
        self._api_key = key
        self.base_url = (base_url or os.environ.get("ALLTERNIT_BASE_URL") or DEFAULT_BASE_URL).rstrip("/")
        self.timeout = timeout

    def create(self, name: Optional[str] = None, account_id: Optional[str] = None, metadata: Optional[Dict[str, Any]] = None) -> Dict[str, Any]:
        body = {k: v for k, v in {"name": name, "account_id": account_id, "metadata": metadata}.items() if v is not None}
        return self._req("POST", "/v1/computers", body)

    def list(self, limit: Optional[int] = None, after: Optional[str] = None) -> Dict[str, Any]:
        return self._req("GET", "/v1/computers" + _qs(limit=limit, after=after))

    def get(self, computer_id: str) -> Dict[str, Any]:
        return self._req("GET", f"/v1/computers/{quote(computer_id)}")

    def start(self, computer_id: str) -> Dict[str, Any]:
        return self._req("POST", f"/v1/computers/{quote(computer_id)}/start", {})

    def stop(self, computer_id: str) -> Dict[str, Any]:
        return self._req("POST", f"/v1/computers/{quote(computer_id)}/stop", {})

    def delete(self, computer_id: str) -> Dict[str, Any]:
        return self._req("DELETE", f"/v1/computers/{quote(computer_id)}")

    def toolset(self, computer_id: str, call: Dict[str, Any]) -> ToolsetResult:
        """Run one member. Action failures return ``is_error: True``; a held call raises ApprovalRequiredError."""
        return self._req("POST", f"/v1/computers/{quote(computer_id)}/toolset", call)

    def toolset_with_approval(
        self,
        computer_id: str,
        call: Dict[str, Any],
        on_approval: Optional[Callable[[Dict[str, Any]], Any]] = None,
    ) -> ToolsetResult:
        """Run one member, answering a 409 ``approval_required`` hold.

        When the server holds the call and ``on_approval`` returns truthy, the
        approval is granted (POST /approvals/{id}) and the same call resent with
        the single-use ``approval_grant``. When ``on_approval`` is missing or
        returns false, the held result resolves (``is_error: True``).
        """
        try:
            return self.toolset(computer_id, call)
        except ApprovalRequiredError as e:
            if not on_approval or not on_approval(e.approval):
                return e.result
            self.approve(computer_id, e.approval["id"])
            return self.toolset(computer_id, {**call, "approval_grant": e.approval["id"]})

    def schema(self, computer_id: str, toolset: str = "computer") -> Dict[str, Any]:
        return self._req("GET", f"/v1/computers/{quote(computer_id)}/toolset/schema" + _qs(toolset=toolset))

    def events(self, computer_id: str, after: Optional[str] = None, limit: Optional[int] = None) -> Dict[str, Any]:
        return self._req("GET", f"/v1/computers/{quote(computer_id)}/events" + _qs(after=after, limit=limit))

    def approve(self, computer_id: str, approval_id: str) -> Dict[str, Any]:
        return self._req("POST", f"/v1/computers/{quote(computer_id)}/approvals/{quote(approval_id)}", {})

    def _req(self, method: str, path: str, body: Any = None) -> Any:
        headers = {"Authorization": f"Bearer {self._api_key}", "Accept": "application/json"}
        data = None
        if body is not None:
            headers["Content-Type"] = "application/json"
            data = json.dumps(body).encode()
        if method == "POST":
            headers["Idempotency-Key"] = str(uuid.uuid4())
        req = Request(self.base_url + path, data=data, method=method, headers=headers)
        try:
            with urlopen(req, timeout=self.timeout) as res:
                text = res.read().decode()
                return json.loads(text) if text else None
        except HTTPError as e:
            text = e.read().decode(errors="replace")
            try:
                payload = json.loads(text) if text else {}
            except ValueError:
                payload = {"error": {"message": text}}
            err = payload.get("error") or {}
            if e.code == 409 and err.get("code") == "approval_required" and payload.get("approval"):
                raise ApprovalRequiredError(err, payload["approval"], payload.get("result") or {"is_error": True, "content": []}, payload) from None
            raise _typed_api_error(e.code, err, payload) from None
        except URLError as e:
            raise AllternitApiError(0, {"type": "connection_error", "code": "connection_error", "message": str(e.reason)}, None) from None


def result_text(r: ToolsetResult) -> str:
    return "\n".join(b["text"] for b in r.get("content", []) if b.get("type") == "text")


def result_image(r: ToolsetResult) -> Optional[Dict[str, str]]:
    for b in r.get("content", []):
        if b.get("type") == "image":
            return {"media_type": b["media_type"], "data": b["data"]}
    return None


def _qs(**q: Any) -> str:
    p = [(k, str(v)) for k, v in q.items() if v not in (None, "")]
    return "?" + urlencode(p) if p else ""


__all__: List[str] = [
    "AllternitComputers", "AllternitApiError", "ApprovalRequiredError", "ComputerBusyError", "SandboxRequiredError",
    "ComputerConflictError", "DEFAULT_BASE_URL", "result_text", "result_image",
]
