"""Tests against a mocked HTTP server. Run: python3 -m unittest discover -s tests"""

import json
import os
import subprocess
import sys
import threading
import time
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import parse_qs, urlparse

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from allternit_platform import (  # noqa: E402
    AllternitPlatform,
    APIError,
    APITimeoutError,
    AuthenticationError,
    ConflictError,
    InternalServerError,
    InvalidRequestError,
    MessageCompleted,
    MessageDelta,
    NotFoundError,
    PermissionDeniedError,
    RateLimitError,
)

SEEN = []


def msg(content):
    return {"id": "cmsg_1", "object": "conversation.message", "conversation_id": "conv_1", "role": "assistant",
            "content": content, "status": "completed", "error": None, "created_at": "2026-10-07T00:00:00Z"}


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def _json(self, status, body, headers=None):
        raw = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        for k, v in (headers or {}).items():
            self.send_header(k, v)
        self.end_headers()
        self.wfile.write(raw)

    def _handle(self):
        n = int(self.headers.get("Content-Length") or 0)
        body = json.loads(self.rfile.read(n)) if n else None
        SEEN.append({"method": self.command, "path": self.path, "headers": dict(self.headers), "body": body})
        u = urlparse(self.path)
        q = parse_qs(u.query)
        p = u.path
        if p == "/v1/agents" and self.command == "POST":
            return self._json(201, {"id": "agent_1", "object": "agent", **body})
        if p == "/v1/agents" and self.command == "GET":
            if "after" not in q:
                return self._json(200, {"data": [{"id": "agent_1"}, {"id": "agent_2"}], "has_more": True, "next_cursor": "c2"})
            return self._json(200, {"data": [{"id": "agent_3"}], "has_more": False, "next_cursor": None})
        if p.startswith("/v1/errors/"):
            status = int(p.rsplit("/", 1)[1])
            headers = {"x-request-id": "req_123"}
            if status == 429:
                headers["Retry-After"] = "7"
            return self._json(status, {"error": {"type": "x_error", "code": f"code_{status}", "message": f"boom {status}",
                                                 "param": "name" if status == 400 else None}}, headers)
        if p == "/v1/slow":
            time.sleep(0.5)
            return self._json(200, {})
        if p == "/v1/usage":
            return self._json(200, {"object": "usage", "data": [], "has_more": False, "next_cursor": None})
        if p == "/v1/conversations/conv_1/messages":
            if not body.get("stream"):
                return self._json(200, msg("Hi there"))
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Connection", "close")
            self.end_headers()
            if body["content"] == "fail":
                chunks = ['event: message.delta\ndata: {"delta":"Hal"}\n\n',
                          'event: error\ndata: {"error":{"type":"api_error","code":"turn_failed","message":"the turn failed"}}\n\n']
            else:
                chunks = [": keep-alive\n\n", 'event: message.delta\ndata: {"delta":"Hi "}\n\n',
                          'event: message.delta\r\ndata: {"delta":"there"}\r\n\r\n',
                          "event: message.completed\ndata: " + json.dumps(msg("Hi there")) + "\n\n"]
            for c in chunks:
                self.wfile.write(c.encode())
                self.wfile.flush()
            self.close_connection = True
            return
        self._json(404, {"error": {"type": "not_found_error", "code": "route", "message": "no route", "param": None}})

    do_GET = do_POST = do_PATCH = do_PUT = do_DELETE = _handle


class ClientTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=cls.server.serve_forever, daemon=True).start()
        cls.base = f"http://127.0.0.1:{cls.server.server_address[1]}"

    @classmethod
    def tearDownClass(cls):
        cls.server.shutdown()

    def client(self):
        return AllternitPlatform("alt_test_abc", base_url=self.base)

    def test_auth_header_and_body(self):
        agent = self.client().agents.create(account_id="acct_1", name="Front desk")
        self.assertEqual(agent["id"], "agent_1")
        last = SEEN[-1]
        self.assertEqual(last["headers"]["Authorization"], "Bearer alt_test_abc")
        self.assertEqual(last["body"], {"account_id": "acct_1", "name": "Front desk"})

    def test_env_key_and_missing_key(self):
        old = os.environ.get("ALLTERNIT_API_KEY")
        try:
            os.environ["ALLTERNIT_API_KEY"] = "alt_test_fromenv"
            AllternitPlatform(base_url=self.base).agents.list()
            self.assertEqual(SEEN[-1]["headers"]["Authorization"], "Bearer alt_test_fromenv")
            del os.environ["ALLTERNIT_API_KEY"]
            with self.assertRaises(ValueError):
                AllternitPlatform(base_url=self.base)
        finally:
            if old is not None:
                os.environ["ALLTERNIT_API_KEY"] = old

    def test_idempotency_key(self):
        c = self.client()
        c.agents.create(account_id="a", name="n")
        k1 = SEEN[-1]["headers"]["Idempotency-Key"]
        c.agents.create(account_id="a", name="n")
        k2 = SEEN[-1]["headers"]["Idempotency-Key"]
        self.assertRegex(k1, r"^[0-9a-f-]{36}$")
        self.assertNotEqual(k1, k2)
        c.agents.create(account_id="a", name="n", idempotency_key="mine-1")
        self.assertEqual(SEEN[-1]["headers"]["Idempotency-Key"], "mine-1")
        c.agents.list()
        self.assertNotIn("Idempotency-Key", SEEN[-1]["headers"])

    def test_error_mapping(self):
        c = self.client()
        cases = [(400, InvalidRequestError), (401, AuthenticationError), (403, PermissionDeniedError),
                 (404, NotFoundError), (409, ConflictError), (422, InvalidRequestError), (429, RateLimitError),
                 (503, InternalServerError), (418, APIError)]
        for status, cls in cases:
            with self.assertRaises(cls) as ctx:
                c.http.request("GET", f"/v1/errors/{status}")
            e = ctx.exception
            self.assertIsInstance(e, APIError)
            self.assertEqual((e.status, e.code, e.message, e.request_id), (status, f"code_{status}", f"boom {status}", "req_123"))
            if status == 400:
                self.assertEqual(e.param, "name")
            if status == 429:
                self.assertEqual(e.retry_after, 7.0)

    def test_timeout(self):
        with self.assertRaises(APITimeoutError):
            self.client().http.request("GET", "/v1/slow", timeout=0.05)

    def test_query_keyword_escape(self):
        self.client().usage.get(group_by="key", from_="2026-10-01")
        q = parse_qs(urlparse(SEEN[-1]["path"]).query)
        self.assertEqual(q, {"group_by": ["key"], "from": ["2026-10-01"]})

    def test_pagination(self):
        c = self.client()
        page = c.agents.list(limit=2)
        self.assertTrue(page["has_more"])
        self.assertIn("limit=2", SEEN[-1]["path"])
        ids = [a["id"] for a in c.agents.list_all(limit=2)]
        self.assertEqual(ids, ["agent_1", "agent_2", "agent_3"])
        self.assertIn("after=c2", SEEN[-1]["path"])

    def test_send_message(self):
        m = self.client().conversations.send_message("conv_1", content="Hello")
        self.assertEqual(m["content"], "Hi there")
        self.assertEqual(SEEN[-1]["body"], {"content": "Hello"})

    def test_stream(self):
        stream = self.client().conversations.stream("conv_1", content="Hello")
        events = list(stream)
        self.assertEqual(SEEN[-1]["body"], {"content": "Hello", "stream": True})
        self.assertEqual(SEEN[-1]["headers"]["Accept"], "text/event-stream")
        self.assertEqual(events[:2], [MessageDelta("Hi "), MessageDelta("there")])
        self.assertIsInstance(events[2], MessageCompleted)
        self.assertEqual(stream.text, "Hi there")
        self.assertEqual(stream.final_message["content"], "Hi there")
        self.assertEqual(self.client().conversations.stream("conv_1", content="x").until_done()["id"], "cmsg_1")

    def test_stream_error_event(self):
        deltas = []
        with self.assertRaises(APIError) as ctx:
            for ev in self.client().conversations.stream("conv_1", content="fail"):
                deltas.append(ev.delta)
        self.assertEqual(deltas, ["Hal"])
        self.assertEqual((ctx.exception.code, ctx.exception.type), ("turn_failed", "api_error"))

    def test_generated_code_is_current(self):
        script = Path(__file__).resolve().parents[3] / "scripts/platform-sdk/generate.py"
        out = subprocess.run([sys.executable, str(script), "--check"], capture_output=True, text=True)
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)


if __name__ == "__main__":
    unittest.main()
