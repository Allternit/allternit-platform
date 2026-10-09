"""
Golden Path: Desktop Terminal Bootstrap (cross-platform)

Registers the terminal/shell execution adapter with ComputerUseExecutor.
Allows running shell commands, reading stdout/stderr, and taking screenshots
of the terminal window. Cross-platform: macOS Terminal, iTerm2, Windows CMD/
PowerShell/Windows Terminal, Linux xterm/gnome-terminal.

Shell commands run directly via asyncio subprocess with a PTY-like
interface. Screenshot, keyboard and coordinate actions go through the
accessibility adapter (Quartz events, macOS).

Usage:
    from golden_paths.bootstrap.desktop_terminal_bootstrap import bootstrap_desktop_terminal
    executor = await bootstrap_desktop_terminal()
"""

from __future__ import annotations

import asyncio
import logging
import os
import platform
import shutil
import subprocess
import sys
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Any, Dict, Optional

logger = logging.getLogger(__name__)


# ---------------------------------------------------------------------------
# Config
# ---------------------------------------------------------------------------

@dataclass
class DesktopTerminalConfig:
    shell: Optional[str] = None          # None = auto-detect
    cwd: Optional[str] = None            # working directory for commands
    timeout_seconds: int = 30            # default command timeout
    env_passthrough: bool = True         # pass parent env to subprocesses
    extra_env: dict = field(default_factory=dict)


# ---------------------------------------------------------------------------
# Shell detection (cross-platform)
# ---------------------------------------------------------------------------

def detect_shell() -> str:
    """Return the best available shell for the current platform."""
    sys_platform = platform.system().lower()

    if sys_platform == "windows":
        pwsh = shutil.which("pwsh") or shutil.which("powershell")
        if pwsh:
            return pwsh
        return "cmd.exe"

    # POSIX
    user_shell = os.environ.get("SHELL", "")
    if user_shell and shutil.which(user_shell):
        return user_shell
    for candidate in ("zsh", "bash", "sh"):
        found = shutil.which(candidate)
        if found:
            return found
    return "/bin/sh"


# ---------------------------------------------------------------------------
# TerminalAdapter
# ---------------------------------------------------------------------------

class TerminalAdapter:
    """
    desktop.terminal adapter — runs shell commands and captures output.

    Registered as "desktop.terminal" in the ComputerUseExecutor.

    Supported action types:
      screenshot     — capture desktop screenshot (via the accessibility adapter)
      cursor_position — get mouse position
      type           — send text to a running process (if process is tracked)
      key            — press a key (via the accessibility adapter)
      left_click     — mouse click (via the accessibility adapter)
      shell          — run a shell command and return stdout/stderr/exit_code
                       (action_type="shell" is an extension beyond Claude's 9)
    """

    def __init__(self, cfg: DesktopTerminalConfig) -> None:
        self._cfg = cfg
        self._shell = cfg.shell or detect_shell()
        self._env = os.environ.copy() if cfg.env_passthrough else {}
        self._env.update(cfg.extra_env)
        if cfg.cwd:
            self._env["PWD"] = cfg.cwd

    @property
    def adapter_id(self) -> str:
        return "desktop.terminal"

    @property
    def family(self) -> str:
        return "desktop"

    async def initialize(self) -> None:
        logger.info("[terminal-bootstrap] using shell: %s", self._shell)

    async def close(self) -> None:
        pass

    async def health_check(self) -> bool:
        try:
            proc = await asyncio.create_subprocess_exec(
                self._shell, "-c", "echo ok",
                stdout=asyncio.subprocess.PIPE,
                stderr=asyncio.subprocess.PIPE,
                env=self._env,
            )
            stdout, _ = await asyncio.wait_for(proc.communicate(), timeout=5.0)
            return b"ok" in stdout
        except Exception:
            return False

    async def capabilities(self):
        try:
            sys.path.insert(0, os.path.join(os.path.dirname(__file__), "../.."))
            from core.base_adapter import AdapterCapabilities
            return AdapterCapabilities(
                adapter_id=self.adapter_id,
                family=self.family,
                dom_tree=False,
                vision_required=False,
                code_execution=True,
                platform=platform.system().lower(),
            )
        except ImportError:
            return None

    async def execute(self, action, session_id: str, run_id: str):
        sys.path.insert(0, os.path.join(os.path.dirname(__file__), "../.."))
        from core.base_adapter import ResultEnvelope

        env = ResultEnvelope(
            run_id=run_id,
            session_id=session_id,
            adapter_id=self.adapter_id,
            family=self.family,
            mode="execute",
            action=action.action_type,
            target=action.target,
            status="running",
            started_at=datetime.now(timezone.utc).isoformat(),
        )

        try:
            result_data = await self._dispatch(action)
            env.status = "completed"
            env.extracted_content = result_data
            env.completed_at = datetime.now(timezone.utc).isoformat()
        except Exception as exc:
            logger.warning("[terminal] %s error: %s", action.action_type, exc)
            env.status = "failed"
            env.error = {"code": "TERMINAL_ERROR", "message": str(exc)}
            env.completed_at = datetime.now(timezone.utc).isoformat()

        return env

    async def _dispatch(self, action) -> Dict[str, Any]:
        p = action.parameters
        at = action.action_type

        # Shell command execution (primary terminal use case)
        if at == "shell" or at == "execute":
            cmd = action.target or p.get("command", "")
            timeout = int(p.get("timeout_seconds", self._cfg.timeout_seconds))
            return await self._run_shell(cmd, timeout=timeout)

        # Type text — send to a running process if available, else keyboard
        if at == "type":
            text = p.get("text", action.target or "")
            return await self._keyboard_type(text)

        if at == "key":
            key = p.get("key", action.target or "")
            return await self._keyboard_key(key)

        if at == "screenshot":
            return await self._screenshot()

        if at == "cursor_position":
            return await self._cursor_position()

        if at in ("left_click", "right_click", "double_click", "middle_click"):
            x, y = int(p.get("x", 0)), int(p.get("y", 0))
            return await self._mouse_click(x, y, at)

        if at == "scroll":
            x = int(p.get("x", 0))
            y_pos = int(p.get("y", 0))
            dy = int(p.get("deltaY", p.get("delta", 0)))
            return await self._scroll(x, y_pos, dy)

        if at == "left_click_drag":
            return await self._drag(
                int(p.get("startX", 0)), int(p.get("startY", 0)),
                int(p.get("endX", 0)), int(p.get("endY", 0)),
            )

        if at == "wait":
            ms = int(p.get("ms", 1000))
            await asyncio.sleep(ms / 1000)
            return {"waited_ms": ms}

        raise ValueError(f"Unsupported terminal action: {at!r}")

    # ── Shell execution ───────────────────────────────────────────────────────

    async def _run_shell(self, command: str, timeout: int = 30) -> Dict[str, Any]:
        sys_platform = platform.system().lower()
        if sys_platform == "windows":
            args = [self._shell, "/c", command]
        else:
            args = [self._shell, "-c", command]

        proc = await asyncio.create_subprocess_exec(
            *args,
            stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.PIPE,
            env=self._env,
            cwd=self._cfg.cwd,
        )
        try:
            stdout, stderr = await asyncio.wait_for(proc.communicate(), timeout=float(timeout))
        except asyncio.TimeoutError:
            proc.kill()
            return {
                "exit_code": -1,
                "stdout": "",
                "stderr": f"Command timed out after {timeout}s",
                "command": command,
            }

        return {
            "exit_code": proc.returncode,
            "stdout": stdout.decode(errors="replace"),
            "stderr": stderr.decode(errors="replace"),
            "command": command,
        }

    # ── Native desktop helpers (accessibility adapter, macOS) ──────────────────

    def _native(self):
        adapter = getattr(self, "_native_adapter", None)
        if adapter is None:
            sys.path.insert(0, os.path.join(os.path.dirname(__file__), "../.."))
            from adapters.desktop.accessibility_adapter import AccessibilityAdapter
            adapter = self._native_adapter = AccessibilityAdapter()
        return adapter

    async def _native_exec(self, command: str, params: Dict[str, Any]) -> Dict[str, Any]:
        result = await self._native().execute(command, params)
        if not result.get("success"):
            raise RuntimeError(result.get("error") or f"native desktop {command} failed")
        return result

    async def _screenshot(self) -> Dict[str, Any]:
        result = await self._native_exec("take_screenshot", {})
        return {"data_url": f"data:image/png;base64,{result['image_b64']}"}

    async def _cursor_position(self) -> Dict[str, Any]:
        pos = self._native().cursor_position()
        if pos is None:
            raise RuntimeError("cursor_position needs the macOS accessibility adapter")
        return {"x": pos[0], "y": pos[1]}

    async def _keyboard_type(self, text: str) -> Dict[str, Any]:
        await self._native_exec("type_text", {"text": text})
        return {"chars_typed": len(text)}

    async def _keyboard_key(self, key: str) -> Dict[str, Any]:
        if "+" in key:
            await self._native_exec("key_combo", {"combo": key.lower()})
        else:
            await self._native_exec("press_key", {"key": key.lower()})
        return {"key": key}

    async def _mouse_click(self, x: int, y: int, action_type: str) -> Dict[str, Any]:
        command = {
            "left_click": "click",
            "right_click": "right_click",
            "double_click": "double_click",
        }.get(action_type)
        if command is None:
            raise RuntimeError(f"{action_type} is not supported by the native desktop adapter")
        await self._native_exec(command, {"x": x, "y": y})
        return {}

    async def _scroll(self, x: int, y_pos: int, dy: int) -> Dict[str, Any]:
        clicks = -(dy // 100) or (-1 if dy > 0 else 1)
        await self._native_exec("scroll", {"x": x, "y": y_pos, "delta_x": 0, "delta_y": clicks})
        return {"deltaY": dy}

    async def _drag(self, sx: int, sy: int, ex: int, ey: int) -> Dict[str, Any]:
        await self._native_exec("drag", {
            "from_x": sx, "from_y": sy, "to_x": ex, "to_y": ey, "duration": 0.3,
        })
        return {}


# ---------------------------------------------------------------------------
# Bootstrap entry point
# ---------------------------------------------------------------------------

async def bootstrap_desktop_terminal(
    cfg: Optional[DesktopTerminalConfig] = None,
) -> "ComputerUseExecutor":
    """
    Initialize shell → Health check → Register → Return ready executor.

    The returned executor supports "shell" action_type (extension) plus
    all coordinate/keyboard actions via the accessibility adapter (macOS).
    """
    sys.path.insert(0, os.path.join(os.path.dirname(__file__), "../.."))
    from core.computer_use_executor import ComputerUseExecutor

    cfg = cfg or DesktopTerminalConfig()
    adapter = TerminalAdapter(cfg)
    await adapter.initialize()

    if not await adapter.health_check():
        raise RuntimeError(
            f"Terminal adapter health check failed (shell={adapter._shell}). "
            "Check that the shell is executable and accessible."
        )

    executor = ComputerUseExecutor()
    executor.register("desktop.terminal", adapter)
    logger.info(
        "[terminal-bootstrap] ready — shell=%s  platform=%s",
        adapter._shell, platform.system(),
    )
    return executor


async def _smoke_test():
    executor = await bootstrap_desktop_terminal()
    from core.base_adapter import ActionRequest
    result = await executor.execute(
        ActionRequest(action_type="shell", target="echo hello"),
        session_id="smoke",
        run_id="smoke-001",
    )
    print(f"status={result.status}  stdout={result.extracted_content.get('stdout', '').strip()!r}")
    assert result.status == "completed"
    assert "hello" in (result.extracted_content or {}).get("stdout", "")
    print("[terminal-bootstrap] smoke test passed")


if __name__ == "__main__":
    logging.basicConfig(level=logging.INFO)
    asyncio.run(_smoke_test())
