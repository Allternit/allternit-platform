"""HTTP transport shared by every generated resource: auth, Idempotency-Key,
timeouts, error mapping, cursor pagination and server-sent events.
Standard library only."""

from __future__ import annotations

import json
import os
import socket
import uuid
from dataclasses import dataclass
from typing import Any, Callable, Dict, Iterator, Optional
from urllib.error import HTTPError, URLError
from urllib.parse import urlencode
from urllib.request import Request, urlopen

from ._errors import APIConnectionError, APITimeoutError, error_for

DEFAULT_BASE_URL = "https://api.allternit.com"
DEFAULT_TIMEOUT = 60.0

Page = Dict[str, Any]
"""A list response: ``{"data": [...], "has_more": bool, "next_cursor": str | None}``."""


@dataclass
class SSEEvent:
    """One server-sent event: its ``event:`` name and its ``data:`` payload, JSON-parsed when possible."""

    event: str
    data: Any


class Transport:
    def __init__(
        self,
        api_key: Optional[str] = None,
        *,
        base_url: Optional[str] = None,
        timeout: float = DEFAULT_TIMEOUT,
        default_headers: Optional[Dict[str, str]] = None,
    ) -> None:
        api_key = api_key or os.environ.get("ALLTERNIT_API_KEY")
        if not api_key:
            raise ValueError("No API key. Pass api_key=... or set ALLTERNIT_API_KEY (alt_test_... or alt_live_...).")
        self.api_key = api_key
        self.base_url = (base_url or os.environ.get("ALLTERNIT_BASE_URL") or DEFAULT_BASE_URL).rstrip("/")
        self.timeout = timeout
        self.default_headers = dict(default_headers or {})

    # -- plumbing ---------------------------------------------------------

    def _open(
        self,
        method: str,
        path: str,
        *,
        query: Optional[Dict[str, Any]],
        body: Any,
        accept: str,
        idempotency_key: Optional[str],
        timeout: Optional[float],
        extra_headers: Optional[Dict[str, str]],
    ):
        url = self.base_url + path
        if query:
            pairs = []
            for k, v in query.items():
                if v is None:
                    continue
                for item in v if isinstance(v, (list, tuple)) else [v]:
                    pairs.append((k, "true" if item is True else "false" if item is False else str(item)))
            if pairs:
                url += "?" + urlencode(pairs)
        headers = {
            "Accept": accept,
            "Authorization": f"Bearer {self.api_key}",
            "User-Agent": "allternit-platform-python/0.1.0",
            **self.default_headers,
            **(extra_headers or {}),
        }
        data = None
        if body is not None and method != "GET":
            headers["Content-Type"] = "application/json"
            data = json.dumps(body).encode()
        if method == "POST" and not any(h.lower() == "idempotency-key" for h in headers):
            headers["Idempotency-Key"] = idempotency_key or str(uuid.uuid4())
        req = Request(url, data=data, headers=headers, method=method)
        wait = self.timeout if timeout is None else timeout
        try:
            return urlopen(req, timeout=wait)
        except HTTPError as e:
            raw = e.read().decode("utf-8", "replace")
            try:
                parsed = json.loads(raw) if raw else None
            except ValueError:
                parsed = None
            err = parsed.get("error") if isinstance(parsed, dict) and isinstance(parsed.get("error"), dict) else None
            raise error_for(e.code, err, dict(e.headers.items()), raw[:200] or None) from None
        except (socket.timeout, TimeoutError) as e:
            raise APITimeoutError(f"Request timed out after {wait} s.") from e
        except URLError as e:
            if isinstance(e.reason, (socket.timeout, TimeoutError)):
                raise APITimeoutError(f"Request timed out after {wait} s.") from e
            raise APIConnectionError(f"Could not reach {self.base_url}: {e.reason}") from e

    # -- what generated resources call ------------------------------------

    def request(
        self,
        method: str,
        path: str,
        *,
        query: Optional[Dict[str, Any]] = None,
        body: Any = None,
        idempotency_key: Optional[str] = None,
        timeout: Optional[float] = None,
        extra_headers: Optional[Dict[str, str]] = None,
    ) -> Any:
        resp = self._open(method, path, query=query, body=body, accept="application/json",
                          idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)
        with resp:
            raw = resp.read()
        return json.loads(raw) if raw else None

    def stream_request(
        self,
        method: str,
        path: str,
        *,
        query: Optional[Dict[str, Any]] = None,
        body: Any = None,
        idempotency_key: Optional[str] = None,
        timeout: Optional[float] = None,
        extra_headers: Optional[Dict[str, str]] = None,
    ) -> Iterator[SSEEvent]:
        resp = self._open(method, path, query=query, body=body, accept="text/event-stream",
                          idempotency_key=idempotency_key, timeout=timeout, extra_headers=extra_headers)

        def events() -> Iterator[SSEEvent]:
            with resp:
                yield from parse_sse(iter(resp.readline, b""))

        return events()

    def paginate(self, fetch_page: Callable[[Optional[str]], Page], after: Optional[str] = None) -> Iterator[Any]:
        cursor = after
        while True:
            page = fetch_page(cursor)
            for item in page.get("data") or []:
                yield item
            if not page.get("has_more") or not page.get("next_cursor"):
                return
            cursor = page["next_cursor"]


def parse_sse(lines: Iterator[bytes]) -> Iterator[SSEEvent]:
    """Parse ``text/event-stream`` lines (bytes, newline-terminated) into events."""
    event, data = "message", []
    for raw in lines:
        line = raw.decode("utf-8").rstrip("\r\n")
        if line == "":
            if data:
                payload = "\n".join(data)
                try:
                    parsed: Any = json.loads(payload)
                except ValueError:
                    parsed = payload
                yield SSEEvent(event, parsed)
            event, data = "message", []
            continue
        if line.startswith(":"):
            continue
        field, _, value = line.partition(":")
        if value.startswith(" "):
            value = value[1:]
        if field == "event":
            event = value
        elif field == "data":
            data.append(value)
    if data:
        payload = "\n".join(data)
        try:
            parsed = json.loads(payload)
        except ValueError:
            parsed = payload
        yield SSEEvent(event, parsed)
