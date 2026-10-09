"""Cua Driver engine: one long-lived ``cua-driver mcp`` child over stdio.

The Desktop app runs Cua's daemon (``serve --embedded``) for OS permission
attribution; ``cua-driver mcp --socket`` proxies to it. One persistent MCP
session replaces the old process-per-call ``cua-driver call``.
"""

from __future__ import annotations

import base64
import itertools
import json
import logging
import re
import subprocess
import threading
from concurrent.futures import Future
from typing import Any

from ..element_map import RawNode

log = logging.getLogger("allternit_driver.cua")

# "session 'mcp-...' has ended; tool call 'type_text' was rejected. Call
# start_session with this id to revive it..." — the daemon-side session died
# while our stdio child lives; reconnecting is the revival.
_SESSION_DEAD = re.compile(r"session .+has ended|call start_session", re.IGNORECASE)

# Contract key names (xdotool style, e.g. Return, Page_Up) -> Cua's key names.
_CUA_KEY_NAMES = {
    "enter": "return", "return": "return", "kp_enter": "return", "esc": "escape", "escape": "escape",
    "pgup": "page_up", "pageup": "page_up", "page_up": "page_up", "prior": "page_up",
    "pgdn": "page_down", "pagedown": "page_down", "page_down": "page_down", "next": "page_down",
    "del": "delete", "backspace": "backspace", "space": "space", "tab": "tab",
    "up": "up", "down": "down", "left": "left", "right": "right",
    "home": "home", "end": "end",
}


def _cua_key(name: str) -> str:
    n = name.strip().lower().replace("-", "_")
    if n in _CUA_KEY_NAMES:
        return _CUA_KEY_NAMES[n]
    if n.startswith("f") and n[1:].isdigit():
        return n  # F1..F24
    return n  # Single characters ('a', '0', '=') pass through as-is.


class CuaError(Exception):
    def __init__(self, code: str, message: str) -> None:
        super().__init__(message)
        self.code = code


class CuaEngine:
    name = "cua"

    def __init__(self, executable: str | None, socket: str | None = None, embedded: bool = False, env: dict[str, str] | None = None) -> None:
        self.executable = executable
        self.socket = socket
        self.embedded = embedded
        self.env = env
        self.tools: set[str] = set()
        self._proc: subprocess.Popen[bytes] | None = None
        self._pending: dict[int, Future] = {}
        self._ids = itertools.count(1)
        self._write = threading.Lock()
        self._start = threading.Lock()
        self.error: str | None = None

    # ---- lifecycle ---------------------------------------------------------

    @property
    def available(self) -> bool:
        # A failed start isn't permanent: each call restarts the MCP session
        # if needed, and the router learns from the failures.
        return bool(self.executable)

    def argv(self) -> list[str]:
        args = [self.executable or "cua-driver", "mcp"]
        if self.socket:
            args += ["--socket", self.socket]
            if self.embedded:
                args.append("--embedded")
        else:
            args.append("--direct")
        return args

    def _ensure(self) -> None:
        if self._proc is not None and self._proc.poll() is None:
            return
        with self._start:
            if self._proc is not None and self._proc.poll() is None:
                return
            if not self.executable:
                raise CuaError("engine_unavailable", "Cua Driver isn't installed with this app")
            self._proc = subprocess.Popen(
                self.argv(), stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=self.env,
            )
            threading.Thread(target=self._reader, args=(self._proc,), name="cua-reader", daemon=True).start()
            self._request("initialize", {
                "protocolVersion": "2025-06-18", "capabilities": {},
                "clientInfo": {"name": "allternit-driver", "version": "1"},
            }, 20)
            self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})
            self.tools = {t["name"] for t in self._request("tools/list", {}, 20).get("tools", [])}
            log.info("cua engine ready: %d tools", len(self.tools))

    def start(self) -> None:
        try:
            self._ensure()
            self.error = None
        except Exception as e:  # Reported in status; the arc engine still serves macOS reads.
            self.error = str(e)
            log.warning("cua engine unavailable: %s", e)

    def close(self) -> None:
        proc, self._proc = self._proc, None
        if proc is not None and proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                proc.kill()

    # ---- JSON-RPC over stdio -------------------------------------------------

    def _send(self, msg: dict[str, Any]) -> None:
        assert self._proc is not None and self._proc.stdin is not None
        data = (json.dumps(msg, separators=(",", ":")) + "\n").encode()
        with self._write:
            self._proc.stdin.write(data)
            self._proc.stdin.flush()

    def _reader(self, proc: subprocess.Popen[bytes]) -> None:
        assert proc.stdout is not None
        for line in proc.stdout:
            try:
                msg = json.loads(line)
            except ValueError:
                continue
            fut = self._pending.pop(msg.get("id"), None) if "id" in msg else None
            if fut is not None and not fut.done():
                fut.set_result(msg)
        for fut in list(self._pending.values()):
            if not fut.done():
                fut.set_exception(CuaError("engine_exited", "Cua Driver stopped"))
        self._pending.clear()

    def _request(self, method: str, params: dict[str, Any], timeout: float) -> dict[str, Any]:
        rid = next(self._ids)
        fut: Future = Future()
        self._pending[rid] = fut
        self._send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
        try:
            msg = fut.result(timeout=timeout)
        except TimeoutError:
            self._pending.pop(rid, None)
            raise CuaError("timeout", f"Cua Driver didn't answer {method} in {timeout:.0f}s") from None
        if "error" in msg:
            raise CuaError("rpc_error", str(msg["error"].get("message", msg["error"])))
        return msg.get("result", {})

    def call(self, tool: str, args: dict[str, Any], timeout: float = 15.0) -> dict[str, Any]:
        """One tool call. Returns structuredContent plus ``_text`` and
        ``_images`` (raw bytes); raises CuaError on a refusal."""
        try:
            return self._call(tool, args, timeout)
        except CuaError as e:
            # The daemon-side MCP session can end while our child process
            # lives on (daemon restart, session timeout). Cua then refuses
            # every call with "session ... has ended ... call start_session
            # ... to revive it". Dropping the child and reconnecting starts
            # a fresh session; retry the call once.
            if not _SESSION_DEAD.search(str(e)):
                raise
            log.warning("cua session ended; reconnecting (%s)", e)
            self.close()
            return self._call(tool, args, timeout)

    def _call(self, tool: str, args: dict[str, Any], timeout: float = 15.0) -> dict[str, Any]:
        self._ensure()
        result = self._request("tools/call", {"name": tool, "arguments": args}, timeout)
        self.error = None
        out = dict(result.get("structuredContent") or {})
        texts, images = [], []
        for block in result.get("content", []):
            if block.get("type") == "text":
                texts.append(block.get("text", ""))
            elif block.get("type") == "image" and block.get("data"):
                images.append(base64.b64decode(block["data"]))
        out["_text"] = "\n".join(texts)
        out["_images"] = images
        if result.get("isError"):
            raise CuaError(str(out.get("code") or "refused"), out["_text"] or f"Cua Driver refused {tool}")
        return out

    def has(self, tool: str) -> bool:
        try:
            self._ensure()
        except Exception:
            return False
        return tool in self.tools

    # ---- reads -----------------------------------------------------------------

    def read(self, pid: int, window_id: int, max_elements: int | None = None, query: str | None = None) -> tuple[list[RawNode], dict[str, Any]]:
        args: dict[str, Any] = {"pid": pid, "window_id": window_id, "include_screenshot": False}
        if max_elements:
            args["max_elements"] = max_elements
        out = self.call("get_window_state", args, timeout=20)
        nodes: list[RawNode] = []
        for el in out.get("elements", []):
            f = el.get("frame") or {}
            bounds = (float(f["x"]), float(f["y"]), float(f["w"]), float(f["h"])) if {"x", "y", "w", "h"} <= f.keys() else None
            nodes.append(RawNode(
                key=str(el.get("element_index")),
                role=str(el.get("role") or ""),
                name=str(el.get("label") or el.get("title") or ""),
                value=el.get("value"),
                bounds=bounds,
                parent=str(el["parent_index"]) if el.get("parent_index") is not None else None,
                enabled=el.get("enabled", True) is not False,
                focused=bool(el.get("focused")),
                actions=tuple(a[2:] if str(a).startswith("AX") else str(a) for a in el.get("actions", [])),
                native={"element_token": el.get("element_token"), "element_index": el.get("element_index")},
            ))
        meta = {
            "app": out.get("app_name"),
            "truncated": bool(out.get("truncated")),
            "degraded": out.get("degraded_reason") or (None if nodes else "empty_tree"),
        }
        return nodes, meta

    def windows(self, pid: int | None = None) -> list[dict[str, Any]]:
        args: dict[str, Any] = {"on_screen_only": False}
        if pid:
            args["pid"] = pid
        return list(self.call("list_windows", args).get("windows", []))

    # ---- acting ----------------------------------------------------------------

    def act(self, pid: int, window_id: int, native: dict[str, Any], op: str, value: Any = None, key: str | None = None) -> None:
        target = {"pid": pid, "window_id": window_id, "element_token": native.get("element_token")}
        if op in ("click", "select", "focus"):
            self.call("click", target)
        elif op == "double_click":
            self.call("double_click", target)
        elif op == "right_click":
            self.call("right_click", target)
        elif op == "set_value":
            self.call("set_value", {**target, "value": "" if value is None else str(value)})
        elif op == "press":
            if key and "+" in key:
                raise CuaError("unsupported_op", "press takes one key (Return, Tab, F5); use a run_batch pixel step ({\"pixel\": {\"tool\": \"hotkey\", \"args\": {\"keys\": [...]}}}) for chords")
            self.call("press_key", {**target, "key": _cua_key(key or "return")})
        elif op == "type":
            self.call("type_text", {**target, "text": "" if value is None else str(value)})
        else:
            raise CuaError("unsupported_op", f"{op} isn't an element action")

    def menu(self, pid: int, window_id: int | None, path: list[str]) -> None:
        args: dict[str, Any] = {"pid": pid, "path": path}
        if window_id:
            args["window_id"] = window_id
        self.call("invoke_menu", args)

    def pixel(self, tool: str, args: dict[str, Any]) -> dict[str, Any]:
        return self.call(tool, args)

    def verify(self, pid: int, window_id: int, expect: list[dict[str, Any]], timeout_ms: int = 1500) -> dict[str, Any]:
        return self.call("verify_state", {"pid": pid, "window_id": window_id, "expect": expect, "timeout_ms": timeout_ms, "stable_samples": 1}, timeout=15)

    def screenshot(self, pid: int | None, window_id: int | None) -> bytes:
        if pid and window_id:
            out = self.call("get_window_state", {"pid": pid, "window_id": window_id, "include_accessibility_tree": False, "max_image_dimension": 0})
        else:
            out = self.call("get_desktop_state", {"max_image_dimension": 0})
        if not out["_images"]:
            raise CuaError("no_image", "Cua Driver returned no screenshot")
        return out["_images"][0]

    def zoom(self, pid: int, window_id: int, region: list[float]) -> bytes:
        x1, y1, x2, y2 = region
        out = self.call("zoom", {"pid": pid, "window_id": window_id, "x1": x1, "y1": y1, "x2": x2, "y2": y2})
        if not out["_images"]:
            raise CuaError("no_image", "Cua Driver returned no zoom image")
        return out["_images"][0]

    def parse_visual_regions(self, capture_id: str, options: dict[str, Any] | None = None) -> dict[str, Any]:
        out = self.call("parse_visual_regions", {"capture_id": capture_id, "options": options or {}}, timeout=30)
        out.pop("_images", None)
        return out
