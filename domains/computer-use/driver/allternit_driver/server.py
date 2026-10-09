"""Local JSON-RPC 2.0 API: one JSON object per line.

Unix socket on macOS/Linux (mode 0600 in a 0700 directory). Windows listens
on 127.0.0.1 with a random port written to an endpoint file, and every
request must carry the launch token (``"auth"``).
"""

from __future__ import annotations

import hmac
import json
import logging
import os
import socketserver
import threading
from typing import Any

from .core import PIXEL_TOOLS, Driver, DriverError, handles_cua_error

log = logging.getLogger("allternit_driver.server")

METHODS = {
    "read_ui": "read_ui", "act": "act", "run_batch": "run_batch", "verify": "verify",
    "screenshot": "screenshot", "zoom": "zoom", "wait": "wait", "status": "status",
    "router": "router_table",
}


def dispatch(driver: Driver, method: str, params: dict[str, Any]) -> Any:
    if method == "ping":
        return {"ok": True}
    if method.startswith("pixel_") and method[6:] in PIXEL_TOOLS:
        return driver.pixel(method[6:], params)
    name = METHODS.get(method)
    if name is None:
        raise DriverError("method_not_found", f"unknown method {method}")
    return getattr(driver, name)(params)


def handle_line(driver: Driver, line: bytes, token: str | None) -> bytes | None:
    try:
        req = json.loads(line)
    except ValueError:
        return _encode({"jsonrpc": "2.0", "id": None, "error": {"code": -32700, "message": "parse error"}})
    rid = req.get("id")
    if token is not None and not hmac.compare_digest(str(req.get("auth", "")), token):
        return _encode({"jsonrpc": "2.0", "id": rid, "error": {"code": -32001, "message": "unauthorized"}})
    params = req.get("params") or {}
    try:
        result = dispatch(driver, str(req.get("method", "")), params if isinstance(params, dict) else {})
        resp = {"jsonrpc": "2.0", "id": rid, "result": result}
    except Exception as e:  # Every failure is an answer, never a dropped connection.
        err = handles_cua_error(e)
        if err.code == "error":
            log.exception("%s failed", req.get("method"))
        resp = {"jsonrpc": "2.0", "id": rid, "error": {"code": -32000, "message": str(err), "data": {"code": err.code, **err.data}}}
    return None if rid is None else _encode(resp)


def _encode(msg: dict[str, Any]) -> bytes:
    return (json.dumps(msg, separators=(",", ":"), default=str) + "\n").encode()


def serve(driver: Driver, listen: str, endpoint_file: str | None = None, token: str | None = None) -> None:
    class Handler(socketserver.StreamRequestHandler):
        def handle(self) -> None:
            for line in self.rfile:
                if not line.strip():
                    continue
                out = handle_line(driver, line, token)
                if out is not None:
                    self.wfile.write(out)
                    self.wfile.flush()

    if listen.startswith("unix:"):
        path = listen[5:]
        os.makedirs(os.path.dirname(path), mode=0o700, exist_ok=True)
        if os.path.exists(path):
            os.unlink(path)

        class Server(socketserver.ThreadingMixIn, socketserver.UnixStreamServer):
            daemon_threads = True

        old = os.umask(0o177)
        try:
            server: socketserver.BaseServer = Server(path, Handler)
        finally:
            os.umask(old)
        where = listen
    elif listen.startswith("tcp:"):
        host, _, port = listen[4:].rpartition(":")
        if host not in ("127.0.0.1", "localhost") or token is None:
            raise SystemExit("tcp listening is loopback-only and needs --token")

        class Server(socketserver.ThreadingMixIn, socketserver.TCPServer):  # type: ignore[no-redef]
            daemon_threads = True
            allow_reuse_address = True

        server = Server((host, int(port or 0)), Handler)
        where = f"tcp:{host}:{server.server_address[1]}"
    else:
        raise SystemExit(f"--listen must be unix:<path> or tcp:127.0.0.1:<port>, not {listen}")

    if endpoint_file:
        tmp = endpoint_file + ".tmp"
        with open(tmp, "w", encoding="utf-8") as f:
            f.write(where)
        os.replace(tmp, endpoint_file)
    log.info("allternit driver listening on %s", where)
    print(f"READY {where}", flush=True)
    stop = threading.Event()
    try:
        server.serve_forever(poll_interval=0.5)
    finally:
        stop.set()
        server.server_close()
        if listen.startswith("unix:") and os.path.exists(listen[5:]):
            os.unlink(listen[5:])
