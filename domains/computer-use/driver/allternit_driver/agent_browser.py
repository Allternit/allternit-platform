"""Agent browser launcher for Windows cloud computers.

    python -m allternit_driver.agent_browser start <agent_id> [--browser edge|chrome]
    python -m allternit_driver.agent_browser stop  <agent_id>

``start`` opens Microsoft Edge (preferred, on every Windows image) or Chrome,
headed, in the interactive session with a kept profile at
``C:\\ProgramData\\Allternit\\browser-profiles\\<agent_id>`` and prints one JSON
line ``{agent_id, pid, profile_dir, cdp_port}`` (the same shape as the Linux
``allternit-agent-display`` launcher). ``stop`` closes only that agent's
browser and keeps the profile. Starting an agent id that is already running
returns the running browser.

Windows has one interactive desktop, so one agent drives at a time (the driver
lease is the input owner). The UI Automation engine targets the window by the
``pid`` printed here.
"""

from __future__ import annotations

import argparse
import json
import ntpath
import os
import re
import socket
import subprocess
import sys
import time
from typing import Any

AGENT_ID_RE = re.compile(r"^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$")

_PROGRAM_FILES = [
    os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)"),
    os.environ.get("ProgramFiles", r"C:\Program Files"),
    os.environ.get("LocalAppData", ""),
]
_BROWSERS = {
    "edge": [r"Microsoft\Edge\Application\msedge.exe"],
    "chrome": [r"Google\Chrome\Application\chrome.exe"],
}


class LauncherError(Exception):
    pass


def data_root() -> str:
    return os.environ.get("ALLTERNIT_DATA_DIR", r"C:\ProgramData\Allternit")


def validate_agent_id(agent_id: str) -> str:
    if not AGENT_ID_RE.match(agent_id or "") or ".." in agent_id:
        raise LauncherError(f"invalid agent_id {agent_id!r}: use letters, digits, '.', '_' or '-' (max 64)")
    return agent_id


def profile_dir(agent_id: str) -> str:
    return ntpath.join(data_root(), "browser-profiles", validate_agent_id(agent_id))


def state_file(agent_id: str) -> str:
    return ntpath.join(data_root(), "browser-state", validate_agent_id(agent_id) + ".json")


def build_args(profile: str, cdp_port: int) -> list[str]:
    return [
        f"--user-data-dir={profile}",
        f"--remote-debugging-port={cdp_port}",
        "--remote-debugging-address=127.0.0.1",
        "--force-renderer-accessibility",  # expose the page to UI Automation
        "--no-first-run",
        "--no-default-browser-check",
        "--new-window",
        "about:blank",
    ]


def find_browser(prefer: str = "edge") -> str:
    order = [prefer] + [b for b in _BROWSERS if b != prefer]
    for name in order:
        for root in _PROGRAM_FILES:
            if not root:
                continue
            for rel in _BROWSERS[name]:
                path = ntpath.join(root, rel)
                if os.path.exists(path):
                    return path
    raise LauncherError("no Microsoft Edge or Google Chrome found")


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return int(s.getsockname()[1])


def _alive(pid: int) -> bool:
    out = subprocess.run(["tasklist", "/FI", f"PID eq {pid}", "/NH", "/FO", "CSV"],
                         capture_output=True, text=True).stdout
    return f'"{pid}"' in out


def _read_state(agent_id: str) -> dict[str, Any] | None:
    try:
        with open(state_file(agent_id), encoding="utf-8") as f:
            return json.load(f)
    except (OSError, ValueError):
        return None


def start(agent_id: str, browser: str = "edge") -> dict[str, Any]:
    prof = profile_dir(agent_id)
    prev = _read_state(agent_id)
    if prev and _alive(int(prev["pid"])):
        return prev
    os.makedirs(prof, exist_ok=True)
    os.makedirs(os.path.dirname(state_file(agent_id)), exist_ok=True)
    port = free_port()
    exe = find_browser(browser)
    proc = subprocess.Popen([exe, *build_args(prof, port)], close_fds=True)
    deadline = time.time() + 30
    while time.time() < deadline:  # wait for the DevTools endpoint
        if proc.poll() is not None:
            raise LauncherError(f"the browser exited at start (code {proc.returncode})")
        try:
            socket.create_connection(("127.0.0.1", port), timeout=0.5).close()
            break
        except OSError:
            time.sleep(0.3)
    else:
        proc.kill()
        raise LauncherError("the browser did not open its DevTools port in 30 s")
    info = {"agent_id": agent_id, "pid": proc.pid, "profile_dir": prof, "cdp_port": port}
    with open(state_file(agent_id), "w", encoding="utf-8") as f:
        json.dump(info, f)
    return info


def stop(agent_id: str) -> dict[str, Any]:
    prev = _read_state(agent_id)
    pid = int(prev["pid"]) if prev else None
    if pid and _alive(pid):
        subprocess.run(["taskkill", "/PID", str(pid), "/T"], capture_output=True)  # polite close
        for _ in range(20):
            if not _alive(pid):
                break
            time.sleep(0.5)
        else:
            subprocess.run(["taskkill", "/PID", str(pid), "/T", "/F"], capture_output=True)
    try:
        os.remove(state_file(agent_id))
    except OSError:
        pass
    return {"agent_id": agent_id, "stopped": pid is not None, "profile_dir": profile_dir(agent_id)}


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="allternit-agent-browser")
    ap.add_argument("action", choices=["start", "stop"])
    ap.add_argument("agent_id")
    ap.add_argument("--browser", choices=list(_BROWSERS), default="edge")
    a = ap.parse_args(argv)
    try:
        out = start(a.agent_id, a.browser) if a.action == "start" else stop(a.agent_id)
    except LauncherError as e:
        print(json.dumps({"error": str(e)}), file=sys.stderr)
        return 1
    print(json.dumps(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
