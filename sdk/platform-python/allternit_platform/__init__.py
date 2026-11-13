"""Python SDK for the Allternit Platform API and the self-hosted Agents Server
(``allternit_platform.selfhost``)."""

from ._client import AllternitPlatform, Conversations, ConversationStream, MessageCompleted, MessageDelta, StreamEvent
from ._core import DEFAULT_BASE_URL, SSEEvent, Transport, parse_sse
from ._errors import (
    AllternitError,
    APIConnectionError,
    APIError,
    APITimeoutError,
    AuthenticationError,
    ConflictError,
    InternalServerError,
    InvalidRequestError,
    NotFoundError,
    PaymentRequiredError,
    PermissionDeniedError,
    RateLimitError,
)
from ._generated import NOT_GIVEN
from .selfhost import SelfHostClient, run_custom_tool_worker

try:
    from importlib.metadata import PackageNotFoundError, version as _pkg_version

    __version__ = _pkg_version("allternit-platform")
except Exception:  # not installed (running from a checkout)
    __version__ = "0+unknown"

__all__ = [
    "AllternitPlatform", "Conversations", "ConversationStream", "MessageCompleted", "MessageDelta", "StreamEvent",
    "DEFAULT_BASE_URL", "SSEEvent", "Transport", "parse_sse", "NOT_GIVEN",
    "SelfHostClient", "run_custom_tool_worker",
    "AllternitError", "APIConnectionError", "APIError", "APITimeoutError", "AuthenticationError", "ConflictError",
    "InternalServerError", "InvalidRequestError", "NotFoundError", "PaymentRequiredError", "PermissionDeniedError", "RateLimitError",
]
