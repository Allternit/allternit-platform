"""The Allternit Platform API client.

    from allternit_platform import AllternitPlatform

    client = AllternitPlatform()  # reads ALLTERNIT_API_KEY
    agent = client.agents.create(account_id="acct_...", name="Front desk")

Resources (``client.agents``, ``client.conversations``, ...) are generated
from the OpenAPI file; this module adds the typed conversation stream.
"""

from __future__ import annotations

from dataclasses import dataclass
from typing import Any, Dict, Iterator, Optional, Union

from ._core import DEFAULT_TIMEOUT, SSEEvent, Transport
from ._errors import error_for
from ._generated import ConversationsResource, GeneratedResources


@dataclass
class MessageDelta:
    """A piece of the agent's reply (``message.delta``)."""

    delta: str
    type: str = "message.delta"


@dataclass
class MessageCompleted:
    """The stored reply (``message.completed``)."""

    message: Dict[str, Any]
    type: str = "message.completed"


StreamEvent = Union[MessageDelta, MessageCompleted]


class ConversationStream:
    """The agent's reply as it is written. Iterate it for ``MessageDelta`` and
    ``MessageCompleted`` events; an ``error`` event raises a typed ``APIError``.
    ``final_message`` is set once the stream finishes."""

    def __init__(self, source: Iterator[SSEEvent]) -> None:
        self._source = source
        self._started = False
        self.final_message: Optional[Dict[str, Any]] = None
        self.text = ""

    def __iter__(self) -> Iterator[StreamEvent]:
        if self._started:
            raise RuntimeError("A ConversationStream can only be read once.")
        self._started = True
        for ev in self._source:
            if ev.event == "message.delta":
                delta = str((ev.data or {}).get("delta", "")) if isinstance(ev.data, dict) else str(ev.data)
                self.text += delta
                yield MessageDelta(delta)
            elif ev.event == "message.completed":
                self.final_message = ev.data
                yield MessageCompleted(ev.data)
            elif ev.event == "error":
                body = ev.data.get("error") if isinstance(ev.data, dict) else None
                raise error_for(0, body or {"message": str(ev.data)}, None, "The stream reported an error.")

    def until_done(self) -> Dict[str, Any]:
        """Read the whole stream and return the stored reply."""
        if not self._started:
            for _ in self:
                pass
        if self.final_message is None:
            raise RuntimeError("The stream ended without a message.completed event.")
        return self.final_message


class Conversations(ConversationsResource):
    def stream(
        self,
        id: str,
        *,
        content: str,
        idempotency_key: Optional[str] = None,
        timeout: Optional[float] = None,
        extra_headers: Optional[Dict[str, str]] = None,
    ) -> ConversationStream:
        """Send a message and stream the agent's reply
        (``POST /v1/conversations/{id}/messages`` with ``stream: true``)."""
        return ConversationStream(
            self.send_message_stream(id, content=content, idempotency_key=idempotency_key,
                                     timeout=timeout, extra_headers=extra_headers)
        )


class AllternitPlatform(GeneratedResources):
    """Client for https://api.allternit.com. Every API area is an attribute."""

    conversations: Conversations

    def __init__(
        self,
        api_key: Optional[str] = None,
        *,
        base_url: Optional[str] = None,
        timeout: float = DEFAULT_TIMEOUT,
        default_headers: Optional[Dict[str, str]] = None,
    ) -> None:
        self.http = Transport(api_key, base_url=base_url, timeout=timeout, default_headers=default_headers)
        super().__init__(self.http)
        self.conversations = Conversations(self.http)
