"""Python SDK for the Allternit Platform API."""

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
    PermissionDeniedError,
    RateLimitError,
)
from ._generated import NOT_GIVEN

__version__ = "0.1.0"

__all__ = [
    "AllternitPlatform", "Conversations", "ConversationStream", "MessageCompleted", "MessageDelta", "StreamEvent",
    "DEFAULT_BASE_URL", "SSEEvent", "Transport", "parse_sse", "NOT_GIVEN",
    "AllternitError", "APIConnectionError", "APIError", "APITimeoutError", "AuthenticationError", "ConflictError",
    "InternalServerError", "InvalidRequestError", "NotFoundError", "PermissionDeniedError", "RateLimitError",
]
