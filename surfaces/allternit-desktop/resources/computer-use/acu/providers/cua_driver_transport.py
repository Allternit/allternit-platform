"""Safe local CLI transport for a reviewed Cua Driver installation."""

from __future__ import annotations

import asyncio
import json
import os
import shutil
from dataclasses import dataclass
from pathlib import Path
from typing import Any, Dict, List, Optional


class CuaDriverUnavailableError(RuntimeError):
    pass


class CuaDriverCallError(RuntimeError):
    def __init__(self, tool: str, message: str, *, exit_code: Optional[int] = None) -> None:
        super().__init__(message)
        self.tool = tool
        self.exit_code = exit_code


@dataclass(frozen=True)
class CuaDriverInstallation:
    executable: str
    manifest: Dict[str, Any]

    @property
    def version(self) -> str:
        value = self.manifest.get("version")
        return str(value) if value else "unknown"


# Verified, installed Cua Driver app bundle (macOS). This is the only context
# that satisfies the driver's Computer History admission check, which requires
# the exact executable inside the signed /Applications/CuaDriver.app bundle.
_INSTALLED_CUA_DRIVER_APP = Path("/Applications/CuaDriver.app/Contents/MacOS/cua-driver")
_DEFAULT_INSTALLED_CUA_SOCKET = Path.home() / "Library" / "Caches" / "cua-driver" / "cua-driver.sock"


def _is_installed_cua_driver(executable: str) -> bool:
    return Path(executable).resolve() == _INSTALLED_CUA_DRIVER_APP.resolve()


def discover_cua_driver(explicit_path: Optional[str] = None) -> Optional[str]:
    candidates = [
        explicit_path,
        os.environ.get("ALLTERNIT_CUA_DRIVER_PATH"),
        # Prefer the installed app bundle; it is the only configuration that
        # supports Computer History on macOS.
        str(_INSTALLED_CUA_DRIVER_APP),
        shutil.which("cua-driver"),
        shutil.which("cua-driver-rs"),
        str(Path.home() / ".local" / "bin" / "cua-driver"),
        "/usr/local/bin/cua-driver",
        "/opt/homebrew/bin/cua-driver",
    ]
    for candidate in candidates:
        if not candidate:
            continue
        path = Path(candidate).expanduser()
        if path.is_file() and os.access(path, os.X_OK):
            return str(path.resolve())
    return None


def _default_socket_for_executable(executable: str) -> Optional[str]:
    """Return the default daemon socket for a known installed Cua Driver app."""
    if _is_installed_cua_driver(executable) and _DEFAULT_INSTALLED_CUA_SOCKET.exists():
        return str(_DEFAULT_INSTALLED_CUA_SOCKET)
    return None


def _decode_json_output(stdout: bytes, command: str) -> Dict[str, Any]:
    text = stdout.decode("utf-8", errors="replace").strip()
    if not text:
        return {}
    try:
        value = json.loads(text)
    except json.JSONDecodeError as error:
        # Some releases log a prefix before the final JSON value. Only accept a
        # final line that is itself valid JSON; never guess at arbitrary output.
        lines = [line.strip() for line in text.splitlines() if line.strip()]
        try:
            value = json.loads(lines[-1])
        except (IndexError, json.JSONDecodeError) as final_error:
            raise CuaDriverCallError(command, f"Cua Driver returned non-JSON output: {error}") from final_error
    if not isinstance(value, dict):
        raise CuaDriverCallError(command, "Cua Driver JSON response must be an object")
    return value


class CuaDriverTransport:
    def __init__(self, executable: str, timeout_seconds: float = 30.0) -> None:
        path = Path(executable).expanduser()
        if not path.is_file() or not os.access(path, os.X_OK):
            raise CuaDriverUnavailableError(f"Cua Driver executable is unavailable: {path}")
        self.executable = str(path.resolve())
        self.timeout_seconds = timeout_seconds
        self.socket_path = os.environ.get("ALLTERNIT_CUA_DRIVER_SOCKET") or _default_socket_for_executable(
            self.executable
        )

    @classmethod
    async def discover(
        cls,
        explicit_path: Optional[str] = None,
        timeout_seconds: float = 30.0,
    ) -> Optional["CuaDriverTransport"]:
        executable = discover_cua_driver(explicit_path)
        if executable is None:
            return None
        transport = cls(executable, timeout_seconds)
        await transport.manifest()
        return transport

    async def _run(self, *arguments: str, timeout_seconds: Optional[float] = None) -> Dict[str, Any]:
        allow_telemetry = os.environ.get("ALLTERNIT_ALLOW_UPSTREAM_TELEMETRY", "").strip().lower() in {
            "1", "true", "yes", "on",
        }
        routed_arguments = list(arguments)
        if self.socket_path and arguments and arguments[0] in {"call", "status", "stop", "config", "recording", "history"}:
            routed_arguments.extend(("--socket", self.socket_path))
        process = await asyncio.create_subprocess_exec(
            self.executable,
            *routed_arguments,
            stdin=asyncio.subprocess.DEVNULL,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            env={
                **os.environ,
                "NO_COLOR": "1",
                # Allternit owns consent. Environment override avoids mutating
                # the user's global Cua configuration while defaulting offline.
                "CUA_DRIVER_RS_TELEMETRY_ENABLED": "true" if allow_telemetry else "false",
                "CUA_TELEMETRY_ENABLED": "true" if allow_telemetry else "false",
            },
        )
        try:
            stdout, stderr = await asyncio.wait_for(
                process.communicate(),
                timeout=timeout_seconds or self.timeout_seconds,
            )
        except asyncio.TimeoutError as error:
            process.kill()
            await process.communicate()
            raise CuaDriverCallError(arguments[0], "Cua Driver command timed out") from error
        if process.returncode != 0:
            message = stderr.decode("utf-8", errors="replace").strip() or stdout.decode(
                "utf-8", errors="replace"
            ).strip()
            raise CuaDriverCallError(
                arguments[0],
                message or f"Cua Driver exited with {process.returncode}",
                exit_code=process.returncode,
            )
        return _decode_json_output(stdout, arguments[0])

    async def manifest(self) -> CuaDriverInstallation:
        return CuaDriverInstallation(self.executable, await self._run("manifest"))

    async def status(self) -> Dict[str, Any]:
        return await self._run("status")

    async def history_status(self) -> Dict[str, Any]:
        return await self._run("history", "status")

    async def history_query(
        self,
        *,
        limit: int = 50,
        session_id: Optional[str] = None,
        since_sequence: Optional[int] = None,
        until_sequence: Optional[int] = None,
    ) -> Dict[str, Any]:
        if not isinstance(limit, int) or limit < 1 or limit > 200:
            raise CuaDriverCallError("history_query", "limit must be an integer between 1 and 200")
        if session_id is not None:
            if not isinstance(session_id, str) or len(session_id) < 1 or len(session_id) > 128:
                raise CuaDriverCallError("history_query", "session_id must be 1-128 characters")
        if since_sequence is not None:
            if not isinstance(since_sequence, int) or since_sequence < 1:
                raise CuaDriverCallError("history_query", "since_sequence must be an integer >= 1")
        if until_sequence is not None:
            if not isinstance(until_sequence, int) or until_sequence < 1:
                raise CuaDriverCallError("history_query", "until_sequence must be an integer >= 1")
        if since_sequence is not None and until_sequence is not None and since_sequence > until_sequence:
            raise CuaDriverCallError("history_query", "since_sequence must not exceed until_sequence")

        # The nightly CLI exposes querying as `history list [limit]` with optional
        # `--session`, `--since`, and `--until` flags. The response shape matches
        # the RFC (events array + metadata_only flag).
        command: List[str] = ["history", "list", str(limit)]
        if session_id is not None:
            command.extend(("--session", session_id))
        if since_sequence is not None:
            command.extend(("--since", str(since_sequence)))
        if until_sequence is not None:
            command.extend(("--until", str(until_sequence)))
        return await self._run(*command)

    async def call(
        self,
        tool: str,
        arguments: Dict[str, Any],
        *,
        screenshot_out_file: Optional[str] = None,
        timeout_seconds: Optional[float] = None,
    ) -> Dict[str, Any]:
        command = ["call", tool, json.dumps(arguments, sort_keys=True, separators=(",", ":"))]
        if screenshot_out_file:
            command.extend(("--screenshot-out-file", screenshot_out_file))
        return await self._run(*command, timeout_seconds=timeout_seconds)

    async def history_status(self) -> Dict[str, Any]:
        """Return CUA Driver Computer History operational status."""
        return await self.call("history_status", {})

    async def history_query(
        self,
        *,
        limit: Optional[int] = None,
        session_id: Optional[str] = None,
        since_sequence: Optional[int] = None,
        until_sequence: Optional[int] = None,
    ) -> Dict[str, Any]:
        """Query a bounded, metadata-only slice of CUA Driver Computer History.

        Args match the CUA Driver agent integration RFC. Unknown fields are
        rejected by the driver; this helper only forwards bounded, typed args.
        """
        arguments: Dict[str, Any] = {}
        if limit is not None:
            if not 1 <= limit <= 200:
                raise ValueError("limit must be between 1 and 200")
            arguments["limit"] = limit
        if session_id is not None:
            if not isinstance(session_id, str) or not (1 <= len(session_id) <= 128):
                raise ValueError("session_id must be a string between 1 and 128 characters")
            arguments["session_id"] = session_id
        if since_sequence is not None:
            if not isinstance(since_sequence, int) or since_sequence < 1:
                raise ValueError("since_sequence must be an integer >= 1")
            arguments["since_sequence"] = since_sequence
        if until_sequence is not None:
            if not isinstance(until_sequence, int) or until_sequence < 1:
                raise ValueError("until_sequence must be an integer >= 1")
            arguments["until_sequence"] = until_sequence
        if (
            since_sequence is not None
            and until_sequence is not None
            and since_sequence > until_sequence
        ):
            raise ValueError("since_sequence must not exceed until_sequence")
        return await self.call("history_query", arguments)
