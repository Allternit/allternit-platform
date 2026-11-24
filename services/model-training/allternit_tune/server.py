# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Allternit LLC
"""Engine API v1 server (services/studio-spec/openapi/engine-api.yaml).

    python -m allternit_tune.server --port 0 --token <t> [--data-dir DIR]

Binds 127.0.0.1 only. Every request needs "Authorization: Bearer <token>".
The token can also come from ALLTERNIT_TUNE_TOKEN so it stays out of `ps`.
On start it prints one JSON line {"ready": true, "port": N} to stdout.
Standard library only.
"""

from __future__ import annotations

import argparse
import hmac
import json
import os
import re
import signal
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from typing import Any, Callable, Dict, Optional, Tuple
from urllib.parse import parse_qs, urlparse

from . import ENGINE_ID, ENGINE_NAME, __version__
from .jobs import AlreadyFinished, JobManager, JobRunning
from .spec import MAX_SEQ_LEN, METHODS, SpecError

MAX_BODY = 1 << 20  # 1 MiB; a job spec is a few hundred bytes
_JOB_PATH = re.compile(r"^/v1/jobs/([A-Za-z0-9_-]+)(/events|/cancel)?$")


def engine_card(hardware: Dict[str, Any]) -> Dict[str, Any]:
    """EngineInfo with `fit` for this computer (hardware.probe()): the hardware
    id from the host, the backend, and each method or output it cannot make
    here, with the reason."""
    from .export import find_llama_cpp
    from .hardware import HARDWARE, fit

    llama = find_llama_cpp()
    return {
        "id": ENGINE_ID,
        "name": ENGINE_NAME,
        "license": "Apache-2.0",
        "version": __version__,
        "methods": list(METHODS),
        "hardware": list(HARDWARE),
        "install_state": "installed",
        "limits": {
            "max_seq_len": MAX_SEQ_LEN,
            "mlx_unsupported_methods": ["dpo", "full"],
            "torch": hardware.get("torch"),
            "mlx_lm": hardware.get("mlx_lm"),
            "base_model": "local folder only; the host fetches and verifies weights",
            "gguf": "available" if llama["convert"] else "needs llama.cpp (convert_hf_to_gguf.py)",
        },
        "published": True,
        "process": "embedded",
        "default": True,
        "backends": [
            {"id": "torch", "status": "available"},
            {"id": "mlx", "status": "available"},
        ],
        "notes": "PyTorch on every computer; MLX on a Mac for LoRA and QLoRA. QLoRA uses "
                 "bitsandbytes 4-bit off the Mac. On the CPU only LoRA runs, and slowly.",
        "fit": hardware.get("fit") or fit(hardware["hardware"]),
    }


def make_handler(manager: JobManager, token: str, card: Callable[[], Dict[str, Any]]):
    expected = f"Bearer {token}".encode()

    class Handler(BaseHTTPRequestHandler):
        server_version = f"allternit-tune/{__version__}"
        protocol_version = "HTTP/1.1"

        def log_message(self, fmt: str, *args: Any) -> None:  # quiet by default
            if os.environ.get("ALLTERNIT_TUNE_ACCESS_LOG") == "1":
                super().log_message(fmt, *args)

        def _send(self, status: int, body: Dict[str, Any]) -> None:
            data = json.dumps(body).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def _error(self, status: int, code: str, message: str) -> None:
            self._send(status, {"error": code, "message": message})

        def _authorized(self) -> bool:
            got = (self.headers.get("Authorization") or "").encode()
            if hmac.compare_digest(got, expected):
                return True
            self._error(401, "unauthorized", "missing or wrong bearer token")
            return False

        def _route(self) -> Tuple[str, Optional[str], Optional[str]]:
            path = urlparse(self.path).path.rstrip("/") or "/"
            m = _JOB_PATH.match(path)
            if m:
                return "job", m.group(1), (m.group(2) or "").lstrip("/") or None
            return path, None, None

        def do_GET(self) -> None:  # noqa: N802
            if not self._authorized():
                return
            route, job_id, sub = self._route()
            if route == "/v1/engine":
                return self._send(200, card())
            if route == "job" and sub in (None, "events"):
                job = manager.get(job_id or "")
                if job is None:
                    return self._error(404, "not_found", f"no job {job_id}")
                if sub is None:
                    return self._send(200, job.to_json())
                query = parse_qs(urlparse(self.path).query)
                try:
                    after = int(query.get("after", ["0"])[0])
                except ValueError:
                    return self._error(400, "bad_request", "after must be an integer")
                return self._send(200, job.events_after(max(after, 0)))
            self._error(404, "not_found", f"no route GET {route}")

        def do_POST(self) -> None:  # noqa: N802
            if not self._authorized():
                return
            route, job_id, sub = self._route()
            if route == "/v1/jobs":
                return self._start_job()
            if route == "job" and sub == "cancel":
                self._drain_body()
                if manager.get(job_id or "") is None:
                    return self._error(404, "not_found", f"no job {job_id}")
                try:
                    job = manager.cancel(job_id or "")
                except AlreadyFinished as exc:
                    return self._error(409, "already_finished", str(exc))
                return self._send(200, job.to_json())
            self._error(404, "not_found", f"no route POST {route}")

        def _drain_body(self) -> bytes:
            length = int(self.headers.get("Content-Length") or 0)
            if length > MAX_BODY:
                raise ValueError("body too large")
            return self.rfile.read(length) if length > 0 else b""

        def _start_job(self) -> None:
            try:
                raw = self._drain_body()
            except ValueError:
                self.close_connection = True
                return self._error(413, "too_large", f"job spec is over {MAX_BODY} bytes")
            try:
                spec = json.loads(raw.decode("utf-8") or "null")
            except (ValueError, UnicodeDecodeError) as exc:
                return self._error(422, "invalid_spec", f"job spec is not valid JSON: {exc}")
            try:
                job = manager.submit(spec)
            except SpecError as exc:
                return self._error(422, exc.code, exc.message)
            except JobRunning as exc:
                return self._error(409, "job_running", str(exc))
            self._send(201, getattr(job, "created_json", None) or job.to_json())

    return Handler


def serve(manager: JobManager, token: str, port: int, card: Callable[[], Dict[str, Any]]) -> ThreadingHTTPServer:
    server = ThreadingHTTPServer(("127.0.0.1", port), make_handler(manager, token, card))
    server.daemon_threads = True
    return server


def main(argv: Optional[list] = None) -> int:
    parser = argparse.ArgumentParser(prog="python -m allternit_tune.server", description=__doc__.splitlines()[0])
    parser.add_argument("--port", type=int, default=0, help="loopback port; 0 picks a free one")
    parser.add_argument("--token", default=os.environ.get("ALLTERNIT_TUNE_TOKEN"), help="per-launch bearer token")
    parser.add_argument(
        "--data-dir",
        default=os.environ.get("ALLTERNIT_TUNE_DATA_DIR", str(Path.home() / ".allternit" / "studio" / "allternit-tune")),
        help="where job folders (events, logs; outputs too unless ALLTERNIT_STUDIO_OUTPUT_DIR is set) are written",
    )
    args = parser.parse_args(argv)
    if not args.token:
        parser.error("--token (or ALLTERNIT_TUNE_TOKEN) is required")

    from .hardware import HardwareError, probe

    try:
        hardware = probe()
    except HardwareError as exc:
        parser.error(str(exc))
    output_dir = os.environ.get("ALLTERNIT_STUDIO_OUTPUT_DIR") or None
    manager = JobManager(Path(args.data_dir), host=hardware["host"],
                         output_dir=Path(output_dir) if output_dir else None)
    server = serve(manager, args.token, args.port, lambda: engine_card(hardware))
    print(json.dumps({"ready": True, "port": server.server_address[1], "engine": ENGINE_ID,
                      "version": __version__, "hardware": hardware["hardware"],
                      "backend": hardware["fit"]["backend"], "data_dir": str(manager.data_dir),
                      "output_dir": str(manager.output_dir) if manager.output_dir else None}), flush=True)
    def _on_term(signum: int, frame: Any) -> None:  # noqa: ARG001
        raise KeyboardInterrupt

    # Workers run in their own session; stop them when the host stops us.
    signal.signal(signal.SIGTERM, _on_term)
    try:
        server.serve_forever()
    except KeyboardInterrupt:
        pass
    finally:
        for job in list(manager.jobs.values()):
            if job.state not in ("succeeded", "failed", "cancelled"):
                try:
                    manager.cancel(job.id)
                except AlreadyFinished:
                    pass
        server.server_close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
