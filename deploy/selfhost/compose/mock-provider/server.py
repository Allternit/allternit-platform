#!/usr/bin/env python3
"""A tiny Anthropic Messages API mock for the air-gap test.

Serves POST .../v1/messages (streaming and non-streaming) and
POST .../v1/messages/count_tokens. Standard library only, so it runs in a
plain python image with no network.

Behavior:
  * The newest user turn carries a tool_result block -> a text reply that
    quotes the result ("Tool result: ...").
  * The newest user text contains RUN_BASH -> a tool_use for the `bash` tool
    with `echo ok > /workspace/a && cat /workspace/a`.
  * Anything else -> a canned text reply.

Every request is logged to stdout as one JSON line (model, stream, reply kind)
so the test can see what the server sent.
"""
import json
import os
import sys
import uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(os.environ.get("PORT", "8080"))
BASH_COMMAND = "echo ok > /workspace/a && cat /workspace/a"
CANNED_TEXT = "Hello from the mock provider."


def log(event, **fields):
    sys.stdout.write(json.dumps({"event": event, **fields}) + "\n")
    sys.stdout.flush()


def blocks(content):
    if isinstance(content, str):
        return [{"type": "text", "text": content}]
    if isinstance(content, list):
        return [b for b in content if isinstance(b, dict)]
    return []


def tool_result_text(block):
    inner = block.get("content")
    if isinstance(inner, str):
        return inner
    return " ".join(b.get("text", "") for b in blocks(inner) if b.get("type") == "text")


def decide(body):
    """Return ("tool_use", input) or ("text", text) for the request."""
    messages = body.get("messages") or []
    last_user = next((m for m in reversed(messages) if m.get("role") == "user"), None)
    if last_user is None:
        return "text", CANNED_TEXT
    content = blocks(last_user.get("content"))
    results = [b for b in content if b.get("type") == "tool_result"]
    if results:
        return "text", "Tool result: " + " ".join(tool_result_text(r).strip() for r in results)
    text = " ".join(b.get("text", "") for b in content if b.get("type") == "text")
    if "RUN_BASH" in text and any(t.get("name") == "bash" for t in body.get("tools") or []):
        return "tool_use", {"command": BASH_COMMAND, "description": "Write ok to a file and print it"}
    if "RUN_BASH" in text:
        # The agent has no bash tool: say so instead of calling an unknown tool.
        return "text", "RUN_BASH asked, but the request offered no bash tool."
    return "text", CANNED_TEXT


def usage(output_tokens):
    return {"input_tokens": 10, "output_tokens": output_tokens,
            "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}


def full_message(body, kind, payload):
    msg_id = "msg_mock_" + uuid.uuid4().hex[:16]
    if kind == "tool_use":
        content = [{"type": "tool_use", "id": "toolu_mock_" + uuid.uuid4().hex[:16],
                    "name": "bash", "input": payload}]
        stop = "tool_use"
    else:
        content = [{"type": "text", "text": payload}]
        stop = "end_turn"
    return {"id": msg_id, "type": "message", "role": "assistant",
            "model": body.get("model", "mock-model"), "content": content,
            "stop_reason": stop, "stop_sequence": None, "usage": usage(5)}


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, fmt, *args):  # quiet default access log
        pass

    def _json(self, status, obj):
        data = json.dumps(obj).encode()
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def do_GET(self):
        if self.path.rstrip("/").endswith("/health") or self.path == "/":
            return self._json(200, {"ok": True})
        if self.path.rstrip("/").endswith("/v1/models"):
            return self._json(200, {"data": [{"type": "model", "id": "mock-model",
                                              "display_name": "Mock model",
                                              "created_at": "2026-01-01T00:00:00Z"}],
                                    "has_more": False, "first_id": "mock-model", "last_id": "mock-model"})
        self._json(404, {"type": "error", "error": {"type": "not_found_error", "message": self.path}})

    def do_POST(self):
        length = int(self.headers.get("content-length") or 0)
        raw = self.rfile.read(length) if length else b"{}"
        try:
            body = json.loads(raw or b"{}")
        except ValueError:
            return self._json(400, {"type": "error",
                                    "error": {"type": "invalid_request_error", "message": "bad json"}})
        path = self.path.split("?")[0].rstrip("/")
        if path.endswith("/messages/count_tokens"):
            return self._json(200, {"input_tokens": 10})
        if not path.endswith("/messages"):
            return self._json(404, {"type": "error", "error": {"type": "not_found_error", "message": path}})

        kind, payload = decide(body)
        log("messages", model=body.get("model"), stream=bool(body.get("stream")), reply=kind)
        message = full_message(body, kind, payload)
        if not body.get("stream"):
            return self._json(200, message)
        self._stream(message)

    def _stream(self, message):
        self.send_response(200)
        self.send_header("content-type", "text/event-stream")
        self.send_header("cache-control", "no-cache")
        self.send_header("connection", "close")
        self.end_headers()

        def send(event, data):
            self.wfile.write(f"event: {event}\ndata: {json.dumps(data)}\n\n".encode())
            self.wfile.flush()

        start = dict(message, content=[], stop_reason=None, usage=usage(1))
        send("message_start", {"type": "message_start", "message": start})
        block = message["content"][0]
        if block["type"] == "text":
            send("content_block_start", {"type": "content_block_start", "index": 0,
                                         "content_block": {"type": "text", "text": ""}})
            send("content_block_delta", {"type": "content_block_delta", "index": 0,
                                         "delta": {"type": "text_delta", "text": block["text"]}})
        else:
            send("content_block_start", {"type": "content_block_start", "index": 0,
                                         "content_block": {"type": "tool_use", "id": block["id"],
                                                           "name": block["name"], "input": {}}})
            send("content_block_delta", {"type": "content_block_delta", "index": 0,
                                         "delta": {"type": "input_json_delta",
                                                   "partial_json": json.dumps(block["input"])}})
        send("content_block_stop", {"type": "content_block_stop", "index": 0})
        send("message_delta", {"type": "message_delta",
                               "delta": {"stop_reason": message["stop_reason"], "stop_sequence": None},
                               "usage": {"output_tokens": 5}})
        send("message_stop", {"type": "message_stop"})
        self.close_connection = True


if __name__ == "__main__":
    log("listening", port=PORT)
    ThreadingHTTPServer(("0.0.0.0", PORT), Handler).serve_forever()
