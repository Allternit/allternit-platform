"""Self-hosted client and custom-tool worker against a mocked Agents Server.
Run: python3 -m unittest discover -s tests"""

import json
import subprocess
import sys
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from allternit_platform.selfhost import (  # noqa: E402
    MAX_TOOL_RESULT_BYTES,
    SelfHostClient,
    is_turn_end,
    run_custom_tool_worker,
)

SECRET = "sess-secret-123"


class Server:
    """A tiny in-memory stand-in for the worker routes and the event stream."""

    def __init__(self):
        self.lock = threading.Lock()
        self.reset()

    def reset(self, claims=None, calls=None, stale_on_next=False):
        self.claims = list(claims or [])
        self.calls = list(calls or [])
        self.stale_on_next = stale_on_next
        self.results = []
        self.completed = []
        self.released = []
        self.secret_headers = []
        self.last_event_ids = []
        self.stream_round = 0


STATE = Server()


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *args):
        pass

    def _json(self, status, body=None):
        raw = json.dumps(body).encode() if body is not None else b""
        self.send_response(status)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def _body(self):
        n = int(self.headers.get("content-length") or 0)
        return json.loads(self.rfile.read(n)) if n else {}

    def do_GET(self):
        path = urlparse(self.path).path
        if path == "/v1/sessions/sess_1/events/stream":
            with STATE.lock:
                STATE.last_event_ids.append(self.headers.get("Last-Event-ID"))
                STATE.stream_round += 1
                rnd = STATE.stream_round
            # Round 1 sends seq 1-2 then drops; round 2 replays 2 (must be skipped) and adds 3.
            seqs = [1, 2] if rnd == 1 else [2, 3]
            frames = ": keepalive\n\n"
            for s in seqs:
                ev = {"id": f"evt_{s}", "object": "event", "session_id": "sess_1", "seq": s,
                      "type": "status" if s == 3 else "message",
                      "data": {"status": "idle"} if s == 3 else {"role": "assistant", "content": f"m{s}"},
                      "created_at": "2026-11-12T00:00:00Z"}
                frames += f"id: {s}\nevent: {ev['type']}\ndata: {json.dumps(ev)}\n\n"
            raw = frames.encode()
            self.send_response(200)
            self.send_header("content-type", "text/event-stream")
            self.send_header("content-length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)
            return
        self._json(404, {"error": {"type": "not_found_error", "message": "no", "code": "not_found"}})

    def do_POST(self):
        path = urlparse(self.path).path
        body = self._body()
        with STATE.lock:
            if path == "/v1/environments/env_1/work/claim":
                if not STATE.claims:
                    return self._json(204)
                return self._json(200, STATE.claims.pop(0))
            if path.endswith("/tool_calls/next"):
                STATE.secret_headers.append(self.headers.get("X-Session-Secret"))
                if STATE.stale_on_next:
                    return self._json(409, {"error": {"type": "conflict_error", "message": "stale", "code": "lease_stale"}})
                if not STATE.calls:
                    return self._json(204)
                return self._json(200, STATE.calls.pop(0))
            if path.startswith("/v1/tool_calls/") and path.endswith("/result"):
                STATE.secret_headers.append(self.headers.get("X-Session-Secret"))
                if len(body["output"].encode()) > MAX_TOOL_RESULT_BYTES:
                    return self._json(413, {"error": {"type": "payload_too_large_error", "message": "big", "code": "result_too_large"}})
                STATE.results.append((path.split("/")[3], body))
                return self._json(200, {"tool_call_id": path.split("/")[3], "status": "completed"})
            if path.endswith("/complete"):
                STATE.completed.append((path.split("/")[3], body))
                return self._json(200, {"work_id": path.split("/")[3], "lease_generation": body["lease_generation"],
                                        "lease_expires_at": None, "status": "completed"})
            if path.endswith("/release"):
                STATE.released.append(path.split("/")[3])
                return self._json(200, {"work_id": path.split("/")[3], "lease_generation": body["lease_generation"],
                                        "lease_expires_at": None, "status": "ready"})
            if path.endswith("/renew"):
                return self._json(200, {"work_id": path.split("/")[3], "lease_generation": body["lease_generation"],
                                        "lease_expires_at": "2026-11-12T00:01:00Z", "status": "claimed"})
        self._json(404, {"error": {"type": "not_found_error", "message": "no", "code": "not_found"}})


def claim(work_id="work_1"):
    return {"work_id": work_id, "session_id": "sess_1", "session_secret": SECRET, "lease_generation": 3,
            "lease_expires_at": "2026-11-12T00:01:00Z",
            "environment": {"network": "none", "allowed_hosts": [], "git": {}, "mcp": {}}}


class SelfHostTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.httpd = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        cls.base = f"http://127.0.0.1:{cls.httpd.server_address[1]}"
        threading.Thread(target=cls.httpd.serve_forever, daemon=True).start()

    @classmethod
    def tearDownClass(cls):
        cls.httpd.shutdown()

    def client(self):
        return SelfHostClient("aek_test", base_url=self.base)

    def test_drain_runs_custom_tools_once_per_id_and_completes(self):
        runs = []

        def lookup(inp, ctx):
            runs.append(inp)
            return {"order": inp["id"], "status": "shipped"}

        STATE.reset(claims=[claim()], calls=[
            {"tool_call_id": "tc_1", "name": "lookup_order", "input": {"id": "A1"}},
            {"tool_call_id": "tc_2", "name": "bash", "input": {"command": "ls"}},
            {"tool_call_id": "tc_1", "name": "lookup_order", "input": {"id": "A1"}},  # handed out again
        ])
        run_custom_tool_worker("env_1", {"lookup_order": lookup}, client=self.client(), block_ms=0, drain=True)

        self.assertEqual(runs, [{"id": "A1"}])  # second tc_1 answered from memory
        self.assertEqual([r[0] for r in STATE.results], ["tc_1", "tc_2", "tc_1"])
        self.assertEqual(json.loads(STATE.results[0][1]["output"]), {"order": "A1", "status": "shipped"})
        self.assertTrue(STATE.results[1][1]["is_error"])
        self.assertIn('No handler for tool "bash"', STATE.results[1][1]["output"])
        self.assertEqual(STATE.completed, [("work_1", {"lease_generation": 3, "outcome": "success"})])
        self.assertTrue(all(h == SECRET for h in STATE.secret_headers))

    def test_handler_error_becomes_error_result(self):
        def boom(inp, ctx):
            raise RuntimeError("database down")

        STATE.reset(claims=[claim()], calls=[{"tool_call_id": "tc_9", "name": "q", "input": {}}])
        run_custom_tool_worker("env_1", {"q": boom}, client=self.client(), block_ms=0, drain=True)
        self.assertEqual(STATE.results[0][1], {"output": "database down", "is_error": True})

    def test_oversize_output_is_truncated_under_the_limit(self):
        STATE.reset(claims=[claim()], calls=[{"tool_call_id": "tc_big", "name": "dump", "input": {}}])
        run_custom_tool_worker("env_1", {"dump": lambda i, c: "é" * 200_000}, client=self.client(), block_ms=0, drain=True)
        out = STATE.results[0][1]["output"]
        self.assertLessEqual(len(out.encode()), MAX_TOOL_RESULT_BYTES)
        self.assertIn("[truncated:", out)

    def test_stale_lease_posts_nothing_and_does_not_complete(self):
        STATE.reset(claims=[claim()], stale_on_next=True)
        run_custom_tool_worker("env_1", {}, client=self.client(), block_ms=0, drain=True)
        self.assertEqual(STATE.results, [])
        self.assertEqual(STATE.completed, [])
        self.assertEqual(STATE.released, [])

    def test_drain_with_no_work_returns_after_one_empty_claim(self):
        STATE.reset()
        run_custom_tool_worker("env_1", {}, client=self.client(), block_ms=0, drain=True)
        self.assertEqual(STATE.completed, [])

    def test_stop_hands_the_item_back(self):
        stop = threading.Event()

        def handler(inp, ctx):
            stop.set()
            return "ok"

        STATE.reset(claims=[claim()], calls=[{"tool_call_id": "tc_s", "name": "t", "input": {}}])
        run_custom_tool_worker("env_1", {"t": handler}, client=self.client(), block_ms=0, stop=stop)
        self.assertEqual(STATE.results, [])  # result discarded while stopping
        self.assertEqual(STATE.released, ["work_1"])
        self.assertEqual(STATE.completed, [])

    def test_stream_events_resumes_with_last_event_id(self):
        STATE.reset()
        events = list(self.client().sessions.stream_events("sess_1", until=is_turn_end))
        self.assertEqual([e["seq"] for e in events], [1, 2, 3])
        self.assertEqual(STATE.last_event_ids, [None, "2"])

    def test_generated_selfhost_code_is_current(self):
        root = Path(__file__).resolve().parents[3]
        out = subprocess.run([
            sys.executable, str(root / "scripts/platform-sdk/generate.py"), "--check",
            "--spec", "cmd/allternit-api/openapi/selfhost-v1.yaml",
            "--out", "sdk/platform-ts/src/selfhost",
            "--out-py", "sdk/platform-python/allternit_platform/_selfhost_generated.py",
            "--client-name", "SelfHostClient",
        ], capture_output=True, text=True)
        self.assertEqual(out.returncode, 0, out.stdout + out.stderr)


if __name__ == "__main__":
    unittest.main()
