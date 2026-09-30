from __future__ import annotations

from typing import Any, Optional


class AaiHttpError(Exception):
    def __init__(self, status: int, code: Optional[str], message: str, body: Any = None, retry_after_ms: Optional[int] = None):
        super().__init__(message)
        self.status, self.code, self.body, self.retry_after_ms = status, code, body, retry_after_ms


class ApprovalRequiredError(AaiHttpError):
    """428: a person must approve `approval_id` before the turn reaches the vendor."""

    def __init__(self, message: str, body: Any, approval_id: Optional[str]):
        super().__init__(428, "APPROVAL_REQUIRED", message, body)
        self.approval_id = approval_id


class ConflictError(AaiHttpError):
    """409: BINDING_NOT_READY, REMOTE_CLOSED, CONTEXT_LOST, ALREADY_RESOLVED, ..."""

    def __init__(self, code: Optional[str], message: str, body: Any, approval_id: Optional[str] = None):
        super().__init__(409, code, message, body)
        self.approval_id = approval_id


class RateLimitedError(AaiHttpError):
    def __init__(self, message: str, body: Any, retry_after_ms: Optional[int] = None):
        super().__init__(429, "RATE_LIMITED", message, body, retry_after_ms)


class HumanIntentRequiredError(Exception):
    def __init__(self) -> None:
        super().__init__("respond_approval requires human_intent=True: approvals are answered by a person, never automated")


def to_http_error(status: int, body: Any) -> AaiHttpError:
    b = body if isinstance(body, dict) else {}
    msg = str(b.get("error") or b.get("message") or f"HTTP {status}")
    code = b.get("code") if isinstance(b.get("code"), str) else None
    aid = b.get("approvalId") if isinstance(b.get("approvalId"), str) else None
    retry = b.get("retryAfterMs") if isinstance(b.get("retryAfterMs"), int) else None
    if status == 428:
        return ApprovalRequiredError(msg, body, aid)
    if status == 409:
        return ConflictError(code, msg, body, aid)
    if status == 429:
        return RateLimitedError(msg, body, retry)
    return AaiHttpError(status, code, msg, body, retry)
