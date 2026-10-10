"""Typed errors. Every non-2xx answer becomes an ``APIError`` subclass picked
by HTTP status, carrying the API's ``{"error": {type, code, message, param}}``."""

from __future__ import annotations

from typing import Any, Mapping, Optional


class AllternitError(Exception):
    """Base class for every error this SDK raises."""


class APIError(AllternitError):
    def __init__(
        self,
        status: int,
        body: Optional[Mapping[str, Any]] = None,
        headers: Optional[Mapping[str, str]] = None,
        fallback: Optional[str] = None,
    ) -> None:
        body = body or {}
        self.status = status
        self.type: Optional[str] = body.get("type")
        self.code: Optional[str] = body.get("code")
        self.param: Optional[str] = body.get("param")
        #: Where to fix it (402 ``payment_method_required``: the console billing page).
        self.url: Optional[str] = body.get("url")
        self.headers = dict(headers or {})
        lower = {k.lower(): v for k, v in self.headers.items()}
        self.request_id: Optional[str] = lower.get("x-request-id")
        self.message: str = body.get("message") or fallback or f"HTTP {status}"
        super().__init__(self.message)

    def __repr__(self) -> str:
        return f"{type(self).__name__}(status={self.status}, code={self.code!r}, message={self.message!r})"


class InvalidRequestError(APIError):
    """400 / 422: the request was malformed or a field is invalid."""


class AuthenticationError(APIError):
    """401: missing, invalid or revoked API key."""


class PermissionDeniedError(APIError):
    """403: the key lacks a scope, or is bound to another account."""


# Alias matching the API's `permission_error` type. Not the builtin PermissionError.
PermissionError = PermissionDeniedError  # noqa: A001


class NotFoundError(APIError):
    """404: no such resource (or it belongs to another project)."""


class PaymentRequiredError(APIError):
    """402 ``billing_error``: ``payment_method_required`` (no card on file; open
    ``url``, the console billing page) or ``spend_cap_reached``."""


class ConflictError(APIError):
    """409: e.g. ``conversation_busy`` or an idempotency key reused with a different request."""


class RateLimitError(APIError):
    """429: rate limited. ``retry_after`` is in seconds when the server sent ``Retry-After``."""

    def __init__(self, *args: Any, **kwargs: Any) -> None:
        super().__init__(*args, **kwargs)
        raw = {k.lower(): v for k, v in self.headers.items()}.get("retry-after")
        try:
            self.retry_after: Optional[float] = float(raw) if raw is not None else None
        except ValueError:
            self.retry_after = None


class InternalServerError(APIError):
    """5xx: the API failed, or (``code == "runtime_starting"``) asks you to retry shortly."""


class APIConnectionError(AllternitError):
    """The request never got an HTTP answer (DNS, refused, reset)."""


class APITimeoutError(APIConnectionError):
    """The request took longer than ``timeout``."""


_BY_TYPE = {
    "invalid_request_error": InvalidRequestError,
    "authentication_error": AuthenticationError,
    "billing_error": PaymentRequiredError,
    "permission_error": PermissionDeniedError,
    "not_found_error": NotFoundError,
    "conflict_error": ConflictError,
    "rate_limit_error": RateLimitError,
}


def error_for(
    status: int,
    body: Optional[Mapping[str, Any]],
    headers: Optional[Mapping[str, str]] = None,
    fallback: Optional[str] = None,
) -> APIError:
    """The right error class for a status (0 = an ``error`` event inside a stream)."""
    if status in (400, 422):
        cls: type = InvalidRequestError
    elif status == 401:
        cls = AuthenticationError
    elif status == 402:
        cls = PaymentRequiredError
    elif status == 403:
        cls = PermissionDeniedError
    elif status == 404:
        cls = NotFoundError
    elif status == 409:
        cls = ConflictError
    elif status == 429:
        cls = RateLimitError
    elif status >= 500:
        cls = InternalServerError
    elif status == 0:
        cls = _BY_TYPE.get((body or {}).get("type") or "", APIError)
    else:
        cls = APIError
    return cls(status, body, headers, fallback)
