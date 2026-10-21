"""Loopback HTTP API of the scorer sidecar (standard library only).

    GET  /health     preparation status, progress, engine, model
    POST /prepare    start installing/downloading/loading now
    POST /v1/score   {"context", "options": [text], "kind", "question"}
                     -> {"probs": [...], "timing": {...}, "engine", "model"}

allternit-api's Decision Runtime (`POST /v1/decisions`) is the only intended
caller; it maps option ids, applies thresholds and escalates. When a launch
token is set, every request must carry `Authorization: Bearer <token>`.
"""

from __future__ import annotations

import hmac
import json
import logging
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from typing import Any

from .runtime import Runtime

log = logging.getLogger("allternit_decisions.server")

MAX_OPTIONS = 255
MAX_BODY = 2 * 1024 * 1024


def make_handler(rt: Runtime, token: str | None) -> type[BaseHTTPRequestHandler]:
    class Handler(BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, fmt: str, *args: Any) -> None:  # quiet access log
            log.debug(fmt, *args)

        def _send(self, code: int, body: dict[str, Any]) -> None:
            data = json.dumps(body).encode()
            self.send_response(code)
            self.send_header("content-type", "application/json")
            self.send_header("content-length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def _authorized(self) -> bool:
            if not token:
                return True
            got = self.headers.get("authorization", "")
            return hmac.compare_digest(got.encode(), f"Bearer {token}".encode())

        def do_GET(self) -> None:  # noqa: N802
            if not self._authorized():
                return self._send(401, {"error": "unauthorized"})
            if self.path == "/health":
                return self._send(200, rt.health())
            self._send(404, {"error": "not found"})

        def do_POST(self) -> None:  # noqa: N802
            if not self._authorized():
                return self._send(401, {"error": "unauthorized"})
            length = int(self.headers.get("content-length") or 0)
            if length > MAX_BODY:
                return self._send(413, {"error": "request too large"})
            raw = self.rfile.read(length) if length else b"{}"
            if self.path == "/prepare":
                rt.ensure_started()
                return self._send(200, rt.health())
            if self.path != "/v1/score":
                return self._send(404, {"error": "not found"})
            try:
                req = json.loads(raw)
                context = str(req.get("context") or "")
                options = [str(o) for o in req["options"]]
                kind = str(req.get("kind") or "choice")
                question = req.get("question")
            except (ValueError, KeyError, TypeError) as e:
                return self._send(400, {"error": f"bad request: {e}"})
            if not 2 <= len(options) <= MAX_OPTIONS:
                return self._send(400, {"error": f"options must have 2..{MAX_OPTIONS} entries"})
            if rt.status != "ready":
                rt.ensure_started()  # first use starts preparation
                return self._send(503, {**rt.health(), "error": rt.error or "not ready"})
            try:
                out = rt.scorer.score(context, options, kind, question if isinstance(question, str) else None)
            except ValueError as e:
                return self._send(400, {"error": str(e)})
            except Exception as e:  # noqa: BLE001
                log.exception("score failed")
                return self._send(500, {"error": f"{type(e).__name__}: {e}"})
            self._send(200, {**out, "engine": rt.engine, "model": rt.model})

    return Handler


def serve(rt: Runtime, host: str, port: int, token: str | None, endpoint_file: str | None) -> None:
    httpd = ThreadingHTTPServer((host, port), make_handler(rt, token))
    httpd.daemon_threads = True
    bound = f"http://{host}:{httpd.server_address[1]}"
    if endpoint_file:
        with open(endpoint_file + ".tmp", "w") as f:
            f.write(bound)
        import os

        os.replace(endpoint_file + ".tmp", endpoint_file)
    log.info("listening on %s (engine %s, model %s)", bound, rt.engine, rt.model)
    httpd.serve_forever()
