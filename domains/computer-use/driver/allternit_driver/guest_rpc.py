"""In-guest RPC forwarder (phase D1b): allternit-api -> guest exec -> here -> driver socket.

The control plane can't reach the driver's local socket inside a guest, so it
runs this helper through the guest exec channel (Incus exec, tart exec, or
the Firecracker guest agent's ``execute``) with one base64 JSON-RPC request,
and this helper forwards the line to the driver's socket and prints the
single-line reply on stdout.

Endpoint resolution (first hit wins):
  1. ``ALLTERNIT_DRIVER_GUEST_SOCKET``: ``unix:<path>`` or ``tcp:host:port``.
  2. Linux: the unix socket at ``/run/allternit/driver.sock``.
  3. Windows: the endpoint file ``<ALLTERNIT_DRIVER_GUEST_STATE_DIR>\\endpoint``
     (default ``C:\\ProgramData\\Allternit\\Driver\\endpoint``) written by the
     driver on ``--listen tcp:``; the auth token comes from the adjacent
     ``token`` file (tcp listeners require it).

Transport failures print a JSON-RPC error object and exit 1, so the exec
channel's stderr diagnostics stay meaningful; protocol refusals arrive as
ordinary JSON-RPC errors from the driver itself (exit 0).
"""

from __future__ import annotations

import base64
import json
import os
import socket
import sys

DEFAULT_LINUX_SOCKET = "unix:/run/allternit/driver.sock"
DEFAULT_WINDOWS_STATE_DIR = r"C:\ProgramData\Allternit\Driver"
RECV_LIMIT = 256 * 1024 * 1024


def _fail(message: str) -> "None":
    reply = {"jsonrpc": "2.0", "id": 1, "error": {"code": -32000, "message": message, "data": {"code": "transport"}}}
    print(json.dumps(reply, separators=(",", ":")), flush=True)
    sys.exit(1)


def _endpoint() -> tuple[str, "str | None"]:
    explicit = os.environ.get("ALLTERNIT_DRIVER_GUEST_SOCKET")
    if explicit:
        token = os.environ.get("ALLTERNIT_DRIVER_GUEST_TOKEN")
        return explicit, token
    if os.name == "nt":
        state = os.environ.get("ALLTERNIT_DRIVER_GUEST_STATE_DIR", DEFAULT_WINDOWS_STATE_DIR)
        try:
            with open(os.path.join(state, "endpoint"), encoding="utf-8") as f:
                endpoint = f.read().strip()
        except OSError as e:
            _fail(f"no driver endpoint file in {state}: {e}")
            raise AssertionError("unreachable")
        token = None
        try:
            with open(os.path.join(state, "token"), encoding="utf-8") as f:
                token = f.read().strip() or None
        except OSError:
            pass
        return endpoint, token
    return DEFAULT_LINUX_SOCKET, None


def main() -> None:
    payload = sys.argv[1] if len(sys.argv) > 1 else sys.stdin.read()
    try:
        request = base64.b64decode(payload.strip()).decode("utf-8")
        parsed = json.loads(request)
    except Exception as e:
        _fail(f"the RPC payload isn't valid base64 JSON: {e}")
        return
    endpoint, token = _endpoint()
    if endpoint.startswith("unix:"):
        path = endpoint[5:]
        try:
            sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
            sock.settimeout(600)
            sock.connect(path)
        except OSError as e:
            _fail(f"the driver socket {path} isn't reachable: {e}")
            return
    elif endpoint.startswith("tcp:"):
        host, _, port = endpoint[4:].rpartition(":")
        try:
            sock = socket.create_connection((host or "127.0.0.1", int(port)), timeout=30)
            sock.settimeout(600)
        except OSError as e:
            _fail(f"the driver endpoint {endpoint} isn't reachable: {e}")
            return
        if token:
            parsed["auth"] = token
            request = json.dumps(parsed, separators=(",", ":"))
    else:
        _fail(f"unknown driver endpoint form: {endpoint}")
        return
    try:
        sock.sendall(request.encode("utf-8") + b"\n")
        chunks = []
        total = 0
        while True:
            chunk = sock.recv(65536)
            if not chunk:
                break
            chunks.append(chunk)
            total += len(chunk)
            if total > RECV_LIMIT:
                _fail("the driver's reply exceeded the size limit")
                return
            if b"\n" in chunk:
                break
    except OSError as e:
        _fail(f"the driver connection failed: {e}")
        return
    finally:
        try:
            sock.close()
        except OSError:
            pass
    line = b"".join(chunks).split(b"\n", 1)[0].strip()
    if not line:
        _fail("the driver closed the connection without a reply")
        return
    sys.stdout.write(line.decode("utf-8", "replace") + "\n")
    sys.stdout.flush()


if __name__ == "__main__":
    main()
