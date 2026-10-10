"""The system under test: allternit-api + the Allternit Driver sidecar.

Either point the suite at a running allternit-api (`--api URL`, with
ALLTERNIT_API_TOKEN when it needs auth) or let it boot a private stack on
this Mac from a local build: allternit-api on a loopback port with a scratch
data dir, plus the driver sidecar on a private socket. Nothing here talks to
a production server or a cloud computer.
"""
from __future__ import annotations

import json
import os
import platform
import secrets
import socket
import subprocess
import threading
import time
import urllib.error
import urllib.request
from pathlib import Path
from typing import Optional

REPO = Path(__file__).resolve().parents[4]
CUA_DEFAULT = "/Applications/Allternit Desktop.app/Contents/Resources/computer-use/cua-driver"


def _first(*paths: Optional[str]) -> Optional[str]:
    for p in paths:
        if p and Path(p).exists():
            return p
    return None


def _free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def http_json(method: str, url: str, body=None, headers=None, timeout: float = 130.0):
    data = json.dumps(body).encode() if body is not None else None
    req = urllib.request.Request(url, data=data, method=method)
    req.add_header("content-type", "application/json")
    for k, v in (headers or {}).items():
        req.add_header(k, v)
    try:
        with urllib.request.urlopen(req, timeout=timeout) as r:
            raw = r.read()
            status = r.status
    except urllib.error.HTTPError as e:
        raw, status = e.read(), e.code
    try:
        return status, json.loads(raw or b"null")
    except ValueError:
        return status, {"raw": raw.decode(errors="replace")}


class Stack:
    """A reachable allternit-api with this device registered as a computer."""

    def __init__(self, api: Optional[str], workdir: Path, api_bin: Optional[str] = None):
        self.workdir = workdir
        self.procs: list = []
        self.token = os.environ.get("ALLTERNIT_API_TOKEN", "")
        self.booted = api is None
        if api:
            self.api = api.rstrip("/")
            return
        bin_path = _first(
            api_bin,
            os.environ.get("ALLTERNIT_API_BIN"),
            str(REPO / "target/debug/allternit-api"),
            str(REPO.parent / ".shared-target/debug/allternit-api"),
        )
        if not bin_path:
            raise SystemExit("no allternit-api build found: pass --api URL or --api-bin PATH (or set ALLTERNIT_API_BIN)")
        self._boot(bin_path)

    # -- boot ---------------------------------------------------------------
    def _boot(self, bin_path: str) -> None:
        self.workdir.mkdir(parents=True, exist_ok=True)
        sock = Path("/tmp") / f"allternit-suite-{os.getpid()}.sock"  # AF_UNIX paths are short
        key = f"{platform.system().lower()}-{'arm64' if platform.machine() in ('arm64', 'aarch64') else 'x64'}"
        py = _first(
            os.environ.get("ALLTERNIT_DRIVER_PYTHON"),
            str(REPO / f"surfaces/allternit-desktop/resources/computer-use/driver-python/{key}/bin/python3"),
            str(REPO.parent / f"allternit/surfaces/allternit-desktop/resources/computer-use/driver-python/{key}/bin/python3"),
        )
        if not py:
            raise SystemExit("no driver Python: run surfaces/allternit-desktop/scripts/prepare-allternit-driver.cjs or set ALLTERNIT_DRIVER_PYTHON")
        cua = os.environ.get("ALLTERNIT_CUA_DRIVER_PATH") or CUA_DEFAULT
        drv_log = open(self.workdir / "driver.log", "w")
        self.procs.append(subprocess.Popen(
            [py, "-m", "allternit_driver", "--listen", f"unix:{sock}", "--cua", cua, "--state-dir", str(self.workdir / "driver-state")],
            cwd=REPO / "domains/computer-use/driver", stdout=drv_log, stderr=subprocess.STDOUT,
        ))
        for _ in range(150):
            if sock.exists():
                break
            time.sleep(0.2)
        else:
            self.close()
            raise SystemExit(f"driver sidecar did not start; see {self.workdir / 'driver.log'}")
        port = _free_port()
        self.api = f"http://127.0.0.1:{port}"
        self.desktop_token = secrets.token_hex(16)
        env = dict(os.environ)
        env.update(
            ALLTERNIT_API_HOST="127.0.0.1",
            ALLTERNIT_API_PORT=str(port),
            ALLTERNIT_DATA_DIR=str(self.workdir / "api-data"),
            ALLTERNIT_LOCAL_DEV_BYPASS="1",
            ALLTERNIT_DRIVER_SOCKET=str(sock),
            ALLTERNIT_DESKTOP_ACCESS_TOKEN=self.desktop_token,
        )
        api_log = open(self.workdir / "api.log", "w")
        self.procs.append(subprocess.Popen([bin_path], env=env, stdout=api_log, stderr=subprocess.STDOUT))
        for _ in range(300):
            try:
                status, _ = http_json("GET", f"{self.api}/api/v1/computers", timeout=2)
                if status < 500:
                    break
            except OSError:
                pass
            time.sleep(0.3)
        else:
            self.close()
            raise SystemExit(f"allternit-api did not start; see {self.workdir / 'api.log'}")
        status, body = http_json(
            "POST", f"{self.api}/api/v1/computers/this-device",
            {"id": f"suite-{socket.gethostname()}", "name": "Suite (this Mac)", "platform": platform.system().lower()},
            {"x-allternit-desktop-access-token": self.desktop_token},
        )
        if status >= 300:
            self.close()
            raise SystemExit(f"couldn't register this device: {status} {body}")
        computer = body.get("computer", body) if isinstance(body, dict) else {}
        self.computer_id = computer.get("id", "")
        # The person (not an agent) takes control of this Mac, as the
        # Computers page does; agent input is refused until they have.
        status, body = http_json("POST", f"{self.api}/api/v1/computers/{self.computer_id}/control/take", {}, self.headers())
        if status >= 300:
            self.close()
            raise SystemExit(f"couldn't take control of this device: {status} {body}")
        # Leases expire after 120 s unless renewed; the Computers page renews
        # while the person keeps control, so the suite does the same.
        self._stop = threading.Event()

        def renew() -> None:
            while not self._stop.wait(30):
                try:
                    http_json("POST", f"{self.api}/api/v1/computers/{self.computer_id}/control/take", {}, self.headers(), timeout=10)
                except OSError:
                    pass

        threading.Thread(target=renew, name="control-renew", daemon=True).start()

    # -- calls ----------------------------------------------------------------
    def headers(self) -> dict:
        return {"authorization": f"Bearer {self.token}"} if self.token else {}

    def members(self) -> set:
        status, body = http_json("GET", f"{self.api}/api/v1/computers/this-device/toolset/schema?toolset=computer", headers=self.headers())
        if status != 200 or not isinstance(body, dict):
            return set()
        return {m["name"] for m in body.get("members", []) if m.get("enabled")}

    def close(self) -> None:
        if getattr(self, "_stop", None):
            self._stop.set()
        for p in reversed(self.procs):
            if p.poll() is None:
                p.terminate()
                try:
                    p.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    p.kill()
        self.procs.clear()
