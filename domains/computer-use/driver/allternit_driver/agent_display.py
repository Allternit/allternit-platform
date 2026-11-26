"""One X display and one kept Chrome profile per agent on a Linux guest.

``allternit-agent-display start <agent_id>`` starts a bare Xvfb on the next
free display (``:10`` and up), then Google Chrome *headed* on that display with
``--user-data-dir=/var/lib/allternit/browser-profiles/<agent_id>``. It prints
one JSON object::

    {"agent_id": "a1", "display": ":10", "pid": 4242, "xvfb_pid": 4240,
     "profile_dir": "/var/lib/allternit/browser-profiles/a1",
     "cdp_port": 9310, "vnc_port": 5910, "width": 1280, "height": 800}

``pid`` is the Chrome browser process: the pid the driver's ``read_ui`` /
``act`` / ``screenshot`` take. ``vnc_port`` is set only when x11vnc is
installed (the desktop image); the headless image's live view is the driver's
``screenshot`` with ``display``. ``stop <agent_id>`` ends Chrome, x11vnc and
Xvfb; the profile directory stays, so the agent's logins survive the next run.

Accessibility: every agent Chrome joins the guest's one shared D-Bus session
(``/run/allternit/session-bus.env``), so the one AT-SPI registry the driver
already reads lists every agent's Chrome, told apart by pid. Only X (input
and pixels) is per display; the driver finds a pid's display from
``/proc/<pid>/environ``.

Displays are capped by memory: about ``ALLTERNIT_AGENT_DISPLAY_MB`` (400) per
display after ``ALLTERNIT_AGENT_DISPLAY_RESERVE_MB`` (1536) for the app, the
API and the sidecars, and never more than the memory free right now. Past the
cap ``start`` exits 3 with ``{"error": "display_limit", ...}``.
"""

from __future__ import annotations

import argparse
import fcntl
import json
import os
import re
import shutil
import signal
import socket
import subprocess
import sys
import time
from contextlib import contextmanager
from typing import Any, Callable, Iterator

FIRST_DISPLAY = 10
LAST_DISPLAY = 99
PER_DISPLAY_MB = 400
RESERVE_MB = 1536
CDP_BASE = 9300  # cdp_port = CDP_BASE + display number (9310 for :10)
VNC_BASE = 5900  # vnc_port = VNC_BASE + display number, as computer_screens.rs
STATE_DIR = "/run/allternit/agent-displays"
PROFILES_DIR = "/var/lib/allternit/browser-profiles"
SESSION_BUS_ENV = "/run/allternit/session-bus.env"
# Image defaults (the headless image writes ALLTERNIT_AGENT_DISPLAY_VNC=0: no
# VNC there, its live view is the driver's screenshot of the display).
CONFIG_FILE = "/etc/allternit/agent-display.env"
_CONFIG_KEYS = ("ALLTERNIT_AGENT_DISPLAY_MB", "ALLTERNIT_AGENT_DISPLAY_RESERVE_MB",
                "ALLTERNIT_AGENT_DISPLAY_MAX", "ALLTERNIT_AGENT_DISPLAY_VNC")
X_SOCKET_DIR = "/tmp/.X11-unix"
CHROME_CANDIDATES = ("/usr/bin/google-chrome", "/usr/bin/google-chrome-stable", "/opt/google/chrome/chrome")

_AGENT_ID = re.compile(r"^[A-Za-z0-9_][A-Za-z0-9_.-]{0,63}$")


class DisplayError(Exception):
    """A refusal with a stable code; ``main`` prints it as JSON."""

    def __init__(self, code: str, message: str, **data: Any) -> None:
        super().__init__(message)
        self.code = code
        self.data = data

    def as_json(self) -> dict[str, Any]:
        return {"error": self.code, "message": str(self), **self.data}


# ---- pure parts (unit-tested) ------------------------------------------------------


def check_agent_id(agent_id: str) -> str:
    if not _AGENT_ID.match(agent_id or ""):
        raise DisplayError("bad_agent_id", "agent id must be 1-64 of A-Z a-z 0-9 _ . - and not start with . or -")
    return agent_id


def meminfo_mb(text: str) -> dict[str, int]:
    """``/proc/meminfo`` (lxcfs shows the container's own limit) -> MB."""
    out: dict[str, int] = {}
    for line in text.splitlines():
        key, _, rest = line.partition(":")
        parts = rest.split()
        if parts and parts[0].isdigit():
            out[key.strip()] = int(parts[0]) // 1024
    return out


def display_cap(total_mb: int, available_mb: int, running: int,
                per_display_mb: int = PER_DISPLAY_MB, reserve_mb: int = RESERVE_MB,
                hard_max: int | None = None) -> int:
    """How many agent displays this computer may run in all: what fits after
    the reserve, but never more than ``running`` plus what fits in the memory
    free right now."""
    if per_display_mb <= 0:
        raise ValueError("per_display_mb must be positive")
    by_total = max(0, (total_mb - reserve_mb) // per_display_mb)
    by_free = running + max(0, available_mb // per_display_mb)
    cap = min(by_total, by_free)
    if hard_max is not None:
        cap = min(cap, max(0, hard_max))
    return min(cap, LAST_DISPLAY - FIRST_DISPLAY + 1)


def next_free_display(taken: set[int], in_use: Callable[[int], bool],
                      first: int = FIRST_DISPLAY, last: int = LAST_DISPLAY) -> int:
    """The lowest display number from ``first`` that no agent holds and no X
    server (socket or lock file) is using."""
    for n in range(first, last + 1):
        if n not in taken and not in_use(n):
            return n
    raise DisplayError("display_limit", f"no free X display between :{first} and :{last}")


def chrome_args(chrome: str, display_num: int, profile_dir: str, width: int, height: int,
                as_root: bool, url: str | None = None) -> list[str]:
    """Headed Chrome on its own display: no ``--headless``, no
    ``--disable-extensions`` (the managed-policy extension must load)."""
    args = [
        chrome,
        f"--user-data-dir={profile_dir}",
        f"--remote-debugging-port={CDP_BASE + display_num}",
        "--remote-debugging-address=127.0.0.1",
        "--no-first-run",
        "--no-default-browser-check",
        "--password-store=basic",
        "--force-renderer-accessibility",
        "--disable-gpu",
        "--disable-features=MacAppCodeSignClone",  # workspace rule for every Chrome launch; a no-op on Linux
        "--window-position=0,0",
        f"--window-size={width},{height}",
    ]
    if as_root:
        # Guests run their services as root; Chrome refuses root without it
        # (the Allternit app runs the same way, see the image units).
        args.append("--no-sandbox")
    if url:
        args.append(url)
    return args


# ---- the guest side ----------------------------------------------------------------


def _x_in_use(n: int) -> bool:
    return os.path.exists(f"{X_SOCKET_DIR}/X{n}") or os.path.exists(f"/tmp/.X{n}-lock")


def _alive(pid: int | None) -> bool:
    if not pid:
        return False
    try:
        os.kill(int(pid), 0)
    except ProcessLookupError:
        return False
    except PermissionError:
        return True
    try:  # A zombie child of ours counts as gone.
        with open(f"/proc/{pid}/stat", encoding="utf-8") as f:
            return f.read().split(")")[-1].split()[0] != "Z"
    except OSError:
        return True


def _session_bus() -> str | None:
    try:
        for line in open(SESSION_BUS_ENV, encoding="utf-8"):
            line = line.strip()
            if line.startswith("DBUS_SESSION_BUS_ADDRESS="):
                return line.split("=", 1)[1].strip().strip('"')
    except OSError:
        pass
    return os.environ.get("DBUS_SESSION_BUS_ADDRESS") or None


def load_config(path: str = CONFIG_FILE) -> None:
    """KEY=value lines from the image's config, for known keys the caller's
    environment didn't set."""
    try:
        lines = open(path, encoding="utf-8").read().splitlines()
    except OSError:
        return
    for line in lines:
        key, sep, value = line.strip().partition("=")
        if sep and key in _CONFIG_KEYS and key not in os.environ:
            os.environ[key] = value.strip().strip('"')


def _env(name: str, default: int) -> int:
    try:
        return int(os.environ.get(name, "") or default)
    except ValueError:
        return default


class Displays:
    def __init__(self, state_dir: str = STATE_DIR, profiles_dir: str = PROFILES_DIR) -> None:
        self.state_dir = state_dir
        self.profiles_dir = profiles_dir

    # -- state

    @contextmanager
    def _locked(self) -> Iterator[None]:
        os.makedirs(self.state_dir, mode=0o755, exist_ok=True)
        with open(os.path.join(self.state_dir, ".lock"), "w", encoding="utf-8") as f:
            fcntl.flock(f, fcntl.LOCK_EX)
            try:
                yield
            finally:
                fcntl.flock(f, fcntl.LOCK_UN)

    def _path(self, agent_id: str) -> str:
        return os.path.join(self.state_dir, f"{agent_id}.json")

    def _load(self) -> dict[str, dict[str, Any]]:
        out: dict[str, dict[str, Any]] = {}
        try:
            names = os.listdir(self.state_dir)
        except OSError:
            return out
        for name in names:
            if not name.endswith(".json"):
                continue
            try:
                with open(os.path.join(self.state_dir, name), encoding="utf-8") as f:
                    rec = json.load(f)
                out[str(rec["agent_id"])] = rec
            except (OSError, ValueError, KeyError):
                continue
        return out

    def _prune(self, recs: dict[str, dict[str, Any]]) -> dict[str, dict[str, Any]]:
        """Drop records whose X server died; stop what is left of them."""
        live: dict[str, dict[str, Any]] = {}
        for agent_id, rec in recs.items():
            if _alive(rec.get("xvfb_pid")):
                live[agent_id] = rec
            else:
                self._kill(rec)
                try:
                    os.unlink(self._path(agent_id))
                except OSError:
                    pass
        return live

    # -- commands

    def list(self) -> list[dict[str, Any]]:
        with self._locked():
            return sorted(self._prune(self._load()).values(), key=lambda r: r["display"])

    def start(self, agent_id: str, width: int = 1280, height: int = 800, url: str | None = None,
              vnc: bool = True) -> dict[str, Any]:
        check_agent_id(agent_id)
        with self._locked():
            recs = self._prune(self._load())
            if agent_id in recs:
                rec = recs[agent_id]
                if _alive(rec.get("pid")):
                    return {**rec, "reused": True}
                self._kill(rec)  # Chrome quit: start the agent fresh on the same profile.
                recs.pop(agent_id)
            try:
                mem = meminfo_mb(open("/proc/meminfo", encoding="utf-8").read())
            except OSError:
                mem = {}
            per = _env("ALLTERNIT_AGENT_DISPLAY_MB", PER_DISPLAY_MB)
            cap = display_cap(mem.get("MemTotal", 0), mem.get("MemAvailable", 0), len(recs),
                              per, _env("ALLTERNIT_AGENT_DISPLAY_RESERVE_MB", RESERVE_MB),
                              _env("ALLTERNIT_AGENT_DISPLAY_MAX", LAST_DISPLAY))
            if len(recs) >= cap:
                raise DisplayError(
                    "display_limit",
                    f"this computer can run {cap} agent browser(s) at about {per} MB each and {len(recs)} are running; "
                    "stop one or use a computer with more memory",
                    cap=cap, running=len(recs), per_display_mb=per,
                    total_mb=mem.get("MemTotal", 0), available_mb=mem.get("MemAvailable", 0),
                )
            chrome = next((c for c in CHROME_CANDIDATES if os.access(c, os.X_OK)), None)
            if chrome is None:
                raise DisplayError("chrome_missing", "Google Chrome isn't installed on this computer")
            if shutil.which("Xvfb") is None:
                raise DisplayError("xvfb_missing", "Xvfb isn't installed on this computer")
            taken = {int(str(r["display"]).lstrip(":")) for r in recs.values()}
            n = next_free_display(taken, _x_in_use)
            rec = self._launch(agent_id, n, chrome, width, height, url, vnc)
            with open(self._path(agent_id), "w", encoding="utf-8") as f:
                json.dump(rec, f)
            return rec

    def stop(self, agent_id: str) -> dict[str, Any]:
        check_agent_id(agent_id)
        with self._locked():
            rec = self._load().get(agent_id)
            if rec is None:
                return {"agent_id": agent_id, "stopped": False, "reason": "not_running"}
            self._kill(rec)
            try:
                os.unlink(self._path(agent_id))
            except OSError:
                pass
            return {"agent_id": agent_id, "display": rec["display"], "stopped": True,
                    "profile_dir": rec.get("profile_dir")}

    # -- processes

    def _launch(self, agent_id: str, n: int, chrome: str, width: int, height: int,
                url: str | None, vnc: bool) -> dict[str, Any]:
        display = f":{n}"
        log_dir = os.path.join(self.state_dir, "logs")
        os.makedirs(log_dir, exist_ok=True)
        profile = os.path.join(self.profiles_dir, agent_id)
        os.makedirs(profile, mode=0o700, exist_ok=True)

        def spawn(argv: list[str], log_name: str, env: dict[str, str]) -> subprocess.Popen[bytes]:
            log = open(os.path.join(log_dir, f"{agent_id}-{log_name}.log"), "ab")
            return subprocess.Popen(argv, stdin=subprocess.DEVNULL, stdout=log, stderr=log, env=env,
                                    start_new_session=True, close_fds=True)

        # NO_AT_BRIDGE (any value, in some GTK builds) turns the AT-SPI bridge off.
        base_env = {k: v for k, v in os.environ.items() if k not in ("DISPLAY", "WAYLAND_DISPLAY", "NO_AT_BRIDGE")}
        xvfb = spawn(["Xvfb", display, "-screen", "0", f"{width}x{height}x24", "-nolisten", "tcp", "-noreset"],
                     "xvfb", base_env)
        rec: dict[str, Any] = {"agent_id": agent_id, "display": display, "pid": None, "xvfb_pid": xvfb.pid,
                               "profile_dir": profile, "cdp_port": CDP_BASE + n, "vnc_port": None,
                               "vnc_pid": None, "width": width, "height": height, "started_at": int(time.time())}
        try:
            for _ in range(100):
                if os.path.exists(f"{X_SOCKET_DIR}/X{n}"):
                    break
                if xvfb.poll() is not None:
                    raise DisplayError("xvfb_failed", f"Xvfb {display} exited ({xvfb.returncode})")
                time.sleep(0.05)
            else:
                raise DisplayError("xvfb_failed", f"Xvfb {display} didn't come up")

            env = {**base_env, "DISPLAY": display, "HOME": os.environ.get("HOME") or "/root",
                   "GTK_MODULES": "atk-bridge", "ACCESSIBILITY_ENABLED": "1"}
            bus = _session_bus()
            if bus:
                env["DBUS_SESSION_BUS_ADDRESS"] = bus
                # Wake the a11y bus launcher on the shared session (Chrome asks
                # org.a11y.Bus.GetAddress for it; a fresh :N has no AT_SPI_BUS
                # root property, so the session bus is how it finds it).
                if shutil.which("dbus-send"):
                    subprocess.run(["dbus-send", f"--bus={bus}", "--print-reply", "--dest=org.a11y.Bus",
                                    "/org/a11y/bus", "org.a11y.Bus.GetAddress"],
                                   capture_output=True, timeout=5, check=False)
            argv = chrome_args(chrome, n, profile, width, height, os.geteuid() == 0, url)
            browser = spawn(argv, "chrome", env)
            rec["pid"] = browser.pid
            if vnc and shutil.which("x11vnc"):
                v = spawn(["x11vnc", "-display", display, "-rfbport", str(VNC_BASE + n), "-localhost",
                           "-forever", "-shared", "-nopw", "-quiet"], "vnc", base_env)
                rec["vnc_pid"], rec["vnc_port"] = v.pid, VNC_BASE + n
            # Chrome is up when its DevTools port answers (a headed window
            # exists by then); report a slow start rather than hang the run.
            deadline = time.time() + 20
            while time.time() < deadline:
                if browser.poll() is not None:
                    raise DisplayError("chrome_failed", f"Chrome exited ({browser.returncode}) on {display}")
                try:
                    with socket.create_connection(("127.0.0.1", rec["cdp_port"]), timeout=0.3):
                        break
                except OSError:
                    time.sleep(0.2)
            else:
                rec["warning"] = "chrome_slow_start"
        except BaseException:
            self._kill(rec)
            raise
        return rec

    @staticmethod
    def _kill(rec: dict[str, Any]) -> None:
        pids = [p for p in (rec.get("pid"), rec.get("vnc_pid"), rec.get("xvfb_pid")) if p]
        for pid in pids:  # Each was started in its own session: end the group.
            try:
                os.killpg(int(pid), signal.SIGTERM)
            except (ProcessLookupError, PermissionError, OSError):
                pass
        deadline = time.time() + 5
        while time.time() < deadline and any(_alive(p) for p in pids):
            time.sleep(0.1)
        for pid in pids:
            if _alive(pid):
                try:
                    os.killpg(int(pid), signal.SIGKILL)
                except (ProcessLookupError, PermissionError, OSError):
                    pass


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(prog="allternit-agent-display", description=__doc__.split("\n\n")[0])
    sub = ap.add_subparsers(dest="cmd", required=True)
    s = sub.add_parser("start", help="start (or reuse) the agent's display + Chrome; prints JSON")
    s.add_argument("agent_id")
    s.add_argument("--width", type=int, default=1280)
    s.add_argument("--height", type=int, default=800)
    s.add_argument("--url")
    s.add_argument("--no-vnc", action="store_true", help="don't start x11vnc even when installed")
    t = sub.add_parser("stop", help="stop the agent's display; the profile stays")
    t.add_argument("agent_id")
    sub.add_parser("list", help="running agent displays as JSON")
    a = ap.parse_args(argv)
    load_config()
    d = Displays()
    try:
        if a.cmd == "start":
            if not (320 <= a.width <= 3840 and 240 <= a.height <= 2160):
                raise DisplayError("bad_size", "width/height must be within 320x240 .. 3840x2160")
            vnc = not a.no_vnc and _env("ALLTERNIT_AGENT_DISPLAY_VNC", 1) != 0
            out: Any = d.start(a.agent_id, a.width, a.height, a.url, vnc)
        elif a.cmd == "stop":
            out = d.stop(a.agent_id)
        else:
            out = d.list()
    except DisplayError as e:
        print(json.dumps(e.as_json()))
        return 3 if e.code == "display_limit" else 2
    print(json.dumps(out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
