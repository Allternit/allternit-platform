"""Client for a self-hosted Allternit Agents Server (``allternit-agents-server``).

    from allternit_platform.selfhost import SelfHostClient

    client = SelfHostClient(
        base_url="https://agents.example.internal",  # default http://127.0.0.1:8400
        api_key=os.environ["ALLTERNIT_SERVER_API_KEY"],  # ask_...
    )
    agent = client.agents.create(name="Helper", model="claude-sonnet")

Resources (``client.agents``, ``client.environments``, ``client.sessions``, ...)
are generated from ``cmd/allternit-api/openapi/selfhost-v1.yaml``. This module
adds a typed, resumable session event stream and a custom-tool worker
(``run_custom_tool_worker``) that follows ``docs/SELFHOST_WORKER_PROTOCOL.md``.
The server is yours: nothing here calls ``*.allternit.com``.
"""

from __future__ import annotations

import json
import os
import threading
import time
import uuid
from collections import OrderedDict
from typing import Any, Callable, Dict, Iterator, Mapping, Optional

from ._core import DEFAULT_TIMEOUT, Transport
from ._errors import APIConnectionError, APIError, AuthenticationError, ConflictError, PermissionDeniedError
from ._selfhost_generated import NOT_GIVEN, GeneratedResources, SessionsResource

DEFAULT_SERVER_URL = "http://127.0.0.1:8400"
#: Tool results above this many bytes are refused by the server (413 ``result_too_large``).
MAX_TOOL_RESULT_BYTES = 102_400
#: The protocol says: renew every 20 seconds (the lease lasts 60).
RENEW_EVERY_S = 20.0
#: Session states after which a turn is over.
TURN_END_STATUSES = frozenset({"idle", "interrupted", "error", "archived"})


def is_turn_end(event: Mapping[str, Any]) -> bool:
    """True for a ``status`` event with idle, interrupted, error or archived."""
    if event.get("type") != "status":
        return False
    data = event.get("data") or {}
    return isinstance(data, dict) and data.get("status") in TURN_END_STATUSES


class SelfHostSessions(SessionsResource):
    def stream_events(
        self,
        id: str,
        *,
        after_seq: Optional[int] = None,
        reconnect: bool = True,
        max_reconnects: int = 5,
        until: Optional[Callable[[Dict[str, Any]], bool]] = None,
        timeout: Optional[float] = None,
        extra_headers: Optional[Dict[str, str]] = None,
    ) -> Iterator[Dict[str, Any]]:
        """Session events (dicts shaped like ``Event``) over SSE
        (``GET /v1/sessions/{id}/events/stream``). Keepalive comments are skipped.
        After a dropped connection it reconnects with ``Last-Event-ID``, so no
        event is missed or repeated. Session ``error`` events are yielded, not raised.
        Stops after an event for which ``until(event)`` is true.

            sent = client.sessions.send_message(sid, content="hi")
            for ev in client.sessions.stream_events(sid, after_seq=sent["seq"], until=is_turn_end):
                ...
        """
        last_seq = after_seq
        failures = 0
        while True:
            got_event = False
            try:
                headers = dict(extra_headers or {})
                if last_seq is not None:
                    headers["Last-Event-ID"] = str(last_seq)
                query = {"after_seq": last_seq} if last_seq is not None else None
                frames = self._client.stream_request(
                    "GET", f"/v1/sessions/{_q(id)}/events/stream", query=query,
                    timeout=timeout, extra_headers=headers,
                )
                for frame in frames:
                    event = frame.data
                    if not isinstance(event, dict) or not isinstance(event.get("seq"), int):
                        continue
                    if last_seq is not None and event["seq"] <= last_seq:
                        continue
                    last_seq = event["seq"]
                    got_event = True
                    failures = 0
                    yield event
                    if until is not None and until(event):
                        return
            except (APIConnectionError, OSError):
                if not reconnect:
                    raise
            if not reconnect:
                return
            if not got_event:
                failures += 1
            if failures > max_reconnects:
                raise APIConnectionError(f"The event stream for {id} dropped {failures} times in a row.")
            time.sleep(min(0.5 * 2 ** failures, 10.0))


class SelfHostClient(GeneratedResources):
    """Client for your own Agents Server. Every API area is an attribute."""

    sessions: SelfHostSessions

    def __init__(
        self,
        api_key: Optional[str] = None,
        *,
        base_url: Optional[str] = None,
        timeout: float = DEFAULT_TIMEOUT,
        default_headers: Optional[Dict[str, str]] = None,
    ) -> None:
        """``api_key`` is a server API key (``ask_...``), an environment key
        (``aek_...``, for workers) or an OIDC id token. Defaults to
        ``ALLTERNIT_SERVER_API_KEY``. ``base_url`` defaults to
        ``ALLTERNIT_SERVER_URL`` or http://127.0.0.1:8400."""
        api_key = api_key or os.environ.get("ALLTERNIT_SERVER_API_KEY")
        if not api_key:
            raise ValueError("No key. Pass api_key=... or set ALLTERNIT_SERVER_API_KEY (ask_... server key or aek_... environment key).")
        base_url = base_url or os.environ.get("ALLTERNIT_SERVER_URL") or DEFAULT_SERVER_URL
        self.http = Transport(api_key, base_url=base_url, timeout=timeout, default_headers=default_headers)
        super().__init__(self.http)
        self.sessions = SelfHostSessions(self.http)


# ---------------------------------------------------------------- worker

ToolHandler = Callable[[Dict[str, Any], "ToolCallContext"], Any]
"""Runs one custom tool. Return a string or any JSON value (sent as JSON text).
Raise to send an error result (``is_error: True``) with the message."""


class ToolCallContext:
    """What a handler knows about the call. ``stopped`` is set when the lease is
    lost or the worker stops; the result is then discarded."""

    def __init__(self, work_id: str, session_id: str, tool_call_id: str, name: str,
                 policy: Dict[str, Any], stopped: threading.Event) -> None:
        self.work_id = work_id
        self.session_id = session_id
        self.tool_call_id = tool_call_id
        self.name = name
        #: The environment rules from the claim. Handlers that make network calls should respect them.
        self.policy = policy
        self.stopped = stopped


def truncate_utf8(text: str, max_bytes: int = MAX_TOOL_RESULT_BYTES) -> str:
    """Cut ``text`` to at most ``max_bytes`` UTF-8 bytes, with a note saying it was cut."""
    raw = text.encode("utf-8")
    if len(raw) <= max_bytes:
        return text
    note = f"\n[truncated: {len(raw)} bytes, limit {max_bytes}]"
    keep = max(0, max_bytes - len(note.encode("utf-8")))
    return raw[:keep].decode("utf-8", "ignore") + note


def _to_text(value: Any) -> str:
    if isinstance(value, str):
        return value
    if value is None:
        return ""
    try:
        return json.dumps(value)
    except (TypeError, ValueError):
        return str(value)


def _lease_stale(err: BaseException) -> bool:
    return isinstance(err, ConflictError) and err.code == "lease_stale"


def _lease_gone(err: BaseException) -> bool:
    """The session secret ended or the key was revoked: the item is no longer ours."""
    return _lease_stale(err) or isinstance(err, (AuthenticationError, PermissionDeniedError))


def run_custom_tool_worker(
    environment_id: str,
    tools: Mapping[str, ToolHandler],
    *,
    environment_key: Optional[str] = None,
    base_url: Optional[str] = None,
    client: Optional[SelfHostClient] = None,
    worker_id: Optional[str] = None,
    block_ms: int = 30_000,
    drain: bool = False,
    reclaim_older_than_ms: Optional[int] = None,
    stop: Optional[threading.Event] = None,
    log: Optional[Callable[[str], None]] = None,
) -> None:
    """Claim work for one environment and run custom tools, per the worker protocol.

    Claims a work item with the environment key (``aek_...``, default
    ``ALLTERNIT_ENVIRONMENT_KEY``), renews the lease every 20 seconds, takes the
    session's tool calls with the per-session secret, runs ``tools[name]`` and
    posts the result (truncated to 100 KiB). Runs until ``stop`` is set or
    Ctrl-C (a held item is handed back with ``/release``), or after one item
    with ``drain=True`` (which also returns after one empty claim).

    Built-in sandbox tools (bash, file edits) are run by ``gizzi worker``, not
    here. Give agents whose tools this worker runs their own environment. A
    tool call with no handler here gets an error result. The session secret is
    never logged or passed to a handler.
    """
    say = log or (lambda _line: None)
    block_ms = min(max(block_ms, 0), 30_000)
    worker_id = worker_id or f"custom-tools-{uuid.uuid4().hex[:8]}"
    stop = stop or threading.Event()
    if client is None:
        key = environment_key or os.environ.get("ALLTERNIT_ENVIRONMENT_KEY")
        if not key:
            raise ValueError("No environment key. Pass environment_key=... or set ALLTERNIT_ENVIRONMENT_KEY (aek_...).")
        client = SelfHostClient(key, base_url=base_url)
    request_timeout = block_ms / 1000 + 30
    # One result per tool_call_id: a call handed out again (at-least-once) is
    # answered from here instead of running twice.
    done: "OrderedDict[str, Dict[str, Any]]" = OrderedDict()

    def remember(call_id: str, result: Dict[str, Any]) -> None:
        done[call_id] = result
        while len(done) > 1000:
            done.popitem(last=False)

    def run_call(claim: Dict[str, Any], call: Dict[str, Any], item_stopped: threading.Event) -> Dict[str, Any]:
        cached = done.get(call["tool_call_id"])
        if cached is not None:
            return cached
        handler = tools.get(call["name"])
        if handler is None:
            return {"output": f'No handler for tool "{call["name"]}" on this worker.', "is_error": True}
        ctx = ToolCallContext(claim["work_id"], claim["session_id"], call["tool_call_id"], call["name"],
                              claim.get("environment") or {}, item_stopped)
        try:
            return {"output": _to_text(handler(call.get("input") or {}, ctx)), "is_error": False}
        except Exception as e:  # the handler's failure becomes the tool's error result
            return {"output": str(e) or type(e).__name__, "is_error": True}

    def post_result(call_id: str, result: Dict[str, Any], headers: Dict[str, str]) -> None:
        try:
            client.tool_calls.submit_tool_result(call_id, output=truncate_utf8(result["output"]),
                                                 is_error=result["is_error"], timeout=30, extra_headers=headers)
        except APIError as e:
            if e.status != 413:
                raise
            client.tool_calls.submit_tool_result(call_id, output=truncate_utf8(result["output"], MAX_TOOL_RESULT_BYTES // 2),
                                                 is_error=result["is_error"], timeout=30, extra_headers=headers)

    def run_item(claim: Dict[str, Any]) -> None:
        work_id, generation = claim["work_id"], claim["lease_generation"]
        item_stopped = threading.Event()
        state = {"lost": False}
        failure: Optional[str] = None
        say(f"claimed {work_id} (session {claim['session_id']}, generation {generation})")

        def renew_loop() -> None:
            while not item_stopped.wait(RENEW_EVERY_S):
                try:
                    client.work.renew(work_id, lease_generation=generation, timeout=15)
                except Exception as e:  # keep renewing unless the lease is gone
                    if _lease_stale(e):
                        say(f"lease on {work_id} is stale; another worker owns it. Stopping.")
                        state["lost"] = True
                        item_stopped.set()
                        return
                    say(f"renew failed for {work_id}: {e}")

        renewer = threading.Thread(target=renew_loop, name=f"renew-{work_id}", daemon=True)
        renewer.start()
        # Only the worker holds the secret.
        headers = {"X-Session-Secret": claim["session_secret"]}
        interrupted = False
        try:
            while not item_stopped.is_set() and not stop.is_set():
                try:
                    call = client.tool_calls.next(work_id, block_ms=block_ms, timeout=request_timeout, extra_headers=headers)
                except Exception as e:
                    if _lease_gone(e):
                        state["lost"] = True
                        break
                    raise
                if not call:
                    break  # no tool call in a whole wait window: this item is finished
                result = run_call(claim, call, item_stopped)
                if item_stopped.is_set() or stop.is_set():
                    break  # lease lost or stopping: discard
                try:
                    post_result(call["tool_call_id"], result, headers)
                    remember(call["tool_call_id"], result)
                except Exception as e:
                    if _lease_gone(e):
                        state["lost"] = True
                        break
                    raise
        except KeyboardInterrupt:
            interrupted = True
            stop.set()
        except Exception as e:
            failure = str(e) or type(e).__name__
            say(f"work {work_id} failed: {failure}")
        finally:
            item_stopped.set()

        if state["lost"]:
            return  # another worker owns it: post nothing more
        if stop.is_set():
            try:
                client.work.release(work_id, lease_generation=generation, timeout=15)
                say(f"released {work_id}")
            except Exception:
                pass
            if interrupted:
                raise KeyboardInterrupt
            return
        try:
            if failure:
                client.work.complete(work_id, lease_generation=generation, outcome="failed",
                                     error_message=failure[:2000], timeout=15)
            else:
                client.work.complete(work_id, lease_generation=generation, outcome="success", timeout=15)
            say(f"completed {work_id}")
        except ConflictError as e:
            if not _lease_stale(e):
                raise

    try:
        while not stop.is_set():
            claim = client.work.claim(
                environment_id, worker_id=worker_id, block_ms=block_ms,
                reclaim_older_than_ms=reclaim_older_than_ms if reclaim_older_than_ms is not None else NOT_GIVEN,
                timeout=request_timeout,
            )
            if not claim:
                if drain:
                    return
                continue
            run_item(claim)
            if drain:
                return
    except KeyboardInterrupt:
        return


def _q(v: str) -> str:
    from urllib.parse import quote

    return quote(str(v), safe="")


__all__ = [
    "SelfHostClient", "SelfHostSessions", "DEFAULT_SERVER_URL", "TURN_END_STATUSES", "is_turn_end",
    "run_custom_tool_worker", "ToolCallContext", "ToolHandler", "truncate_utf8",
    "MAX_TOOL_RESULT_BYTES", "RENEW_EVERY_S",
]
