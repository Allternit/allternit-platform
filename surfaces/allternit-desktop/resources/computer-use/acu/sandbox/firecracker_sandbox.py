from __future__ import annotations

import asyncio
import json
import shutil
import time
import uuid
from pathlib import Path
from typing import Any

from .base import BaseSandbox, SandboxConfig, SandboxResult
from .process_sandbox import ProcessSandbox

# Detect Firecracker binary at import time.
FIRECRACKER_AVAILABLE: bool = (
    shutil.which("firecracker") is not None
    or Path("/usr/bin/firecracker").exists()
    or Path("/usr/local/bin/firecracker").exists()
)

_API_SOCKET_TEMPLATE = "/tmp/allternit-firecracker-{sandbox_id}.socket"

# Paths used when configuring the microVM via REST.
# These are well-known locations inside a Firecracker-based rootfs image.
_DEFAULT_KERNEL = "/var/lib/allternit/firecracker/vmlinux"
_DEFAULT_ROOTFS = "/var/lib/allternit/firecracker/rootfs.ext4"

# Guest agent connectivity (see research/FIRECRACKER-GUEST-AGENT-VNC-SPEC.md
# and drivers/firecracker-guest-agent/). Firecracker exposes vsock as one
# host-side Unix socket per VM; which guest port you reach is selected per
# connection via Firecracker's documented "CONNECT <port>\n" / "OK <n>\n"
# handshake -- this is NOT optional, and its absence in the sibling Rust
# driver's `guest_agent_request` (drivers/firecracker/src/lib.rs) looks like
# the same kind of latent gap this whole project has been finding elsewhere;
# flagged in the spec as a follow-up rather than fixed blind here.
_VSOCK_UDS_TEMPLATE = "/tmp/allternit-firecracker-vsock-{sandbox_id}.socket"
_GUEST_CID = 3  # Firecracker's fixed guest CID for a single-VM-per-socket setup
_AGENT_VSOCK_PORT = 10000
_AGENT_READY_TIMEOUT_S = 10.0
_AGENT_READY_POLL_INTERVAL_S = 0.1


class FirecrackerSandbox(BaseSandbox):
    """
    Sandbox using Firecracker microVMs.

    Boot time: ~125 ms. Memory footprint: 5–50 MB per VM.

    Communicates with Firecracker via its REST API over a Unix domain socket.
    Requires the `firecracker` binary and a pre-built kernel + rootfs image.
    Process fallback is disabled by default and requires explicit legacy opt-in.

    REST API surface used:
      PUT /boot-source       — set kernel image + boot args
      PUT /drives/rootfs     — attach root block device
      PUT /machine-config    — set vCPUs + memory
      PUT /vsock             — attach the guest-agent vsock device
      PUT /actions           — InstanceStart
    """

    def __init__(self, config: SandboxConfig) -> None:
        super().__init__(config)
        self._sandbox_id: str = ""
        self._socket_path: str = ""
        self._vsock_uds_path: str = ""
        self._display_vsock_port: int | None = None
        self._fc_proc: asyncio.subprocess.Process | None = None
        self._fallback: ProcessSandbox | None = None

    # ------------------------------------------------------------------
    # Internal helpers
    # ------------------------------------------------------------------

    def _use_fallback(self) -> bool:
        return self._fallback is not None

    def _allow_fallback(self) -> bool:
        return bool(self.config.extra.get("allow_process_fallback", False))

    async def _fallback_or_raise(self, reason: str) -> str:
        if not self._allow_fallback():
            raise RuntimeError(f"Firecracker unavailable and process fallback is disabled: {reason}")
        self._fallback = ProcessSandbox(self.config)
        return await self._fallback.start()

    async def _api(
        self,
        method: str,
        path: str,
        body: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """
        Issue a REST call to the Firecracker API socket.

        Uses httpx with a Unix-domain transport. httpx is an optional dep —
        if absent we fall back to a raw asyncio socket write.
        """
        try:
            import httpx  # noqa: PLC0415

            transport = httpx.AsyncHTTPTransport(uds=self._socket_path)
            async with httpx.AsyncClient(
                transport=transport, base_url="http://localhost"
            ) as client:
                req_kwargs: dict[str, Any] = {}
                if body is not None:
                    req_kwargs["json"] = body
                resp = await client.request(method, path, **req_kwargs)
                if resp.content:
                    return resp.json()
                return {}
        except ImportError:
            # httpx not installed — do a minimal raw HTTP/1.1 request.
            return await self._raw_api(method, path, body)

    async def _raw_api(
        self,
        method: str,
        path: str,
        body: dict[str, Any] | None = None,
    ) -> dict[str, Any]:
        """Minimal HTTP/1.1 over Unix socket when httpx is absent."""
        body_bytes = json.dumps(body).encode() if body else b""
        headers = (
            f"{method} {path} HTTP/1.1\r\n"
            f"Host: localhost\r\n"
            f"Content-Type: application/json\r\n"
            f"Content-Length: {len(body_bytes)}\r\n"
            f"Connection: close\r\n"
            f"\r\n"
        )
        reader, writer = await asyncio.open_unix_connection(self._socket_path)
        writer.write(headers.encode() + body_bytes)
        await writer.drain()
        response_bytes = await reader.read(4096)
        writer.close()
        try:
            _, _, response_body = response_bytes.partition(b"\r\n\r\n")
            return json.loads(response_body.decode())
        except Exception:  # noqa: BLE001
            return {}

    async def _configure_vm(self) -> None:
        """Send boot-source, drives, machine-config, then start the VM."""
        await self._api(
            "PUT",
            "/boot-source",
            {
                "kernel_image_path": _DEFAULT_KERNEL,
                "boot_args": "console=ttyS0 reboot=k panic=1 pci=off",
            },
        )
        await self._api(
            "PUT",
            "/drives/rootfs",
            {
                "drive_id": "rootfs",
                "path_on_host": _DEFAULT_ROOTFS,
                "is_root_device": True,
                "is_read_only": self.config.readonly_root,
            },
        )
        await self._api(
            "PUT",
            "/machine-config",
            {
                "vcpu_count": self.config.vcpus,
                "mem_size_mib": self.config.memory_mb,
            },
        )
        # Vsock device -- required for the guest agent (Execute/StartDisplay/
        # etc.) to be reachable at all. Without this, `run()` has no guest to
        # talk to, exactly the gap that made it fall back to a fake local
        # exec before.
        self._vsock_uds_path = _VSOCK_UDS_TEMPLATE.format(sandbox_id=self._sandbox_id)
        await self._api(
            "PUT",
            "/vsock",
            {
                "vsock_id": "guest-agent",
                "guest_cid": _GUEST_CID,
                "uds_path": self._vsock_uds_path,
            },
        )
        await self._api("PUT", "/actions", {"action_type": "InstanceStart"})

    async def _wait_for_socket(self, timeout_s: float = 3.0) -> bool:
        """Poll until the Firecracker API socket appears on disk."""
        deadline = time.monotonic() + timeout_s
        while time.monotonic() < deadline:
            if Path(self._socket_path).exists():
                return True
            await asyncio.sleep(0.05)
        return False

    # ------------------------------------------------------------------
    # Guest agent transport (see research/FIRECRACKER-GUEST-AGENT-VNC-SPEC.md)
    # ------------------------------------------------------------------

    async def _vsock_connect(self, guest_port: int) -> tuple[asyncio.StreamReader, asyncio.StreamWriter]:
        """Open one connection to a specific guest vsock port.

        Firecracker exposes vsock as a single host-side Unix socket per VM;
        which guest port a given connection reaches is selected with
        Firecracker's documented handshake: send `CONNECT <port>\\n`, then
        read a response line before the socket carries the guest's traffic.
        """
        reader, writer = await asyncio.open_unix_connection(self._vsock_uds_path)
        writer.write(f"CONNECT {guest_port}\n".encode())
        await writer.drain()
        ack = await reader.readline()
        if not ack.startswith(b"OK"):
            writer.close()
            raise RuntimeError(f"vsock CONNECT to guest port {guest_port} failed: {ack!r}")
        return reader, writer

    async def _agent_request(self, request: dict[str, Any], timeout_s: float = 30.0) -> dict[str, Any]:
        """Send one length-prefixed JSON request to the guest agent and
        return the parsed response. One connection per request, matching the
        guest agent's one-thread-per-connection model."""
        reader, writer = await self._vsock_connect(_AGENT_VSOCK_PORT)
        try:
            payload = json.dumps(request).encode()
            writer.write(len(payload).to_bytes(4, "big") + payload)
            await writer.drain()

            len_buf = await asyncio.wait_for(reader.readexactly(4), timeout=timeout_s)
            response_len = int.from_bytes(len_buf, "big")
            response_buf = await asyncio.wait_for(reader.readexactly(response_len), timeout=timeout_s)
            return json.loads(response_buf.decode())
        finally:
            writer.close()

    async def _wait_for_agent(self) -> bool:
        """Poll the guest agent with Ping until it responds or we time out."""
        deadline = time.monotonic() + _AGENT_READY_TIMEOUT_S
        while time.monotonic() < deadline:
            try:
                response = await self._agent_request(
                    {"type": "ping", "version": "1.0.0"}, timeout_s=2.0
                )
                if response.get("type") == "pong":
                    return True
            except Exception:  # noqa: BLE001
                pass
            await asyncio.sleep(_AGENT_READY_POLL_INTERVAL_S)
        return False

    async def start_display(self, width: int = 1280, height: int = 800) -> int:
        """Start a virtual display inside the guest. Returns the vsock port a
        VNC client should tunnel to. Raises on failure -- fail closed, no
        "looks like it worked" degrade."""
        response = await self._agent_request(
            {"type": "start_display", "width": width, "height": height}, timeout_s=15.0
        )
        if response.get("type") == "display_started":
            self._display_vsock_port = int(response["vnc_vsock_port"])
            return self._display_vsock_port
        raise RuntimeError(f"start_display failed: {response.get('message', response)}")

    async def stop_display(self) -> None:
        if self._display_vsock_port is None:
            return
        try:
            await self._agent_request({"type": "stop_display"})
        finally:
            self._display_vsock_port = None

    # ------------------------------------------------------------------
    # BaseSandbox interface
    # ------------------------------------------------------------------

    async def start(self) -> str:
        if not FIRECRACKER_AVAILABLE:
            return await self._fallback_or_raise("binary not found")

        self._sandbox_id = str(uuid.uuid4())[:8]
        self._socket_path = _API_SOCKET_TEMPLATE.format(sandbox_id=self._sandbox_id)

        # Launch the Firecracker process.
        try:
            self._fc_proc = await asyncio.create_subprocess_exec(
                "firecracker",
                "--api-sock",
                self._socket_path,
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
            )
        except FileNotFoundError:
            return await self._fallback_or_raise("binary disappeared during launch")

        # Wait for the API socket to appear.
        socket_ready = await self._wait_for_socket()
        if not socket_ready:
            await self._kill_fc()
            return await self._fallback_or_raise("API socket did not become ready")

        # Configure and boot the microVM.
        try:
            await self._configure_vm()
        except Exception:  # noqa: BLE001
            # Kernel/rootfs images absent in dev — fall back gracefully.
            await self._kill_fc()
            return await self._fallback_or_raise("VM boot configuration failed")

        # Wait for the guest agent to come up before declaring the sandbox
        # ready -- otherwise the first run() would have to discover
        # unreadiness itself, and this is where a "boots but agent isn't
        # there" rootfs (the exact bug this whole thing fixes) gets caught.
        if not await self._wait_for_agent():
            await self._kill_fc()
            return await self._fallback_or_raise(
                "guest agent did not become ready — rootfs may be missing allternit-guest-agent"
            )

        return self._sandbox_id

    async def run(
        self,
        command: list[str],
        env: dict[str, str] | None = None,
    ) -> SandboxResult:
        if self._use_fallback():
            assert self._fallback is not None
            return await self._fallback.run(command, env)

        # Real in-guest execution via the guest agent -- replaces the
        # previous behavior of either raising or silently running the
        # command on a bare local ProcessSandbox and stamping this sandbox's
        # id on the result.
        t0 = time.monotonic()
        try:
            response = await self._agent_request(
                {
                    "type": "execute",
                    "command": command,
                    "env_vars": env or {},
                    "working_dir": None,
                    "stdin_data": None,
                }
            )
        except Exception as exc:  # noqa: BLE001
            duration_ms = (time.monotonic() - t0) * 1000
            return SandboxResult(
                success=False,
                exit_code=-1,
                stdout="",
                stderr=f"Guest agent request failed: {exc}",
                duration_ms=duration_ms,
                sandbox_id=self._sandbox_id,
            )

        duration_ms = (time.monotonic() - t0) * 1000
        if response.get("type") == "error":
            return SandboxResult(
                success=False,
                exit_code=-1,
                stdout="",
                stderr=str(response.get("message", "unknown guest agent error")),
                duration_ms=duration_ms,
                sandbox_id=self._sandbox_id,
            )

        exit_code = int(response.get("exit_code", -1))
        return SandboxResult(
            success=exit_code == 0,
            exit_code=exit_code,
            stdout=(response.get("stdout") or ""),
            stderr=(response.get("stderr") or ""),
            duration_ms=duration_ms,
            sandbox_id=self._sandbox_id,
        )

    async def stop(self) -> None:
        if self._use_fallback():
            assert self._fallback is not None
            await self._fallback.stop()
            return

        if self._display_vsock_port is not None:
            try:
                await self.stop_display()
            except Exception:  # noqa: BLE001
                pass

        # Graceful shutdown via REST, then kill the process.
        try:
            await self._api("PUT", "/actions", {"action_type": "SendCtrlAltDel"})
        except Exception:  # noqa: BLE001
            pass

        await self._kill_fc()

        # Clean up the socket file.
        try:
            Path(self._socket_path).unlink(missing_ok=True)
        except Exception:  # noqa: BLE001
            pass

    async def _kill_fc(self) -> None:
        if self._fc_proc is None:
            return
        try:
            self._fc_proc.terminate()
            await asyncio.wait_for(self._fc_proc.wait(), timeout=3.0)
        except Exception:  # noqa: BLE001
            try:
                self._fc_proc.kill()
            except Exception:  # noqa: BLE001
                pass
        self._fc_proc = None
