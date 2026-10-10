"""The driver's handle on its vision worker.

Nothing heavy ships with the app. On the first vision read the driver installs
the pinned packages (requirements-vision.txt; plus requirements-vision-mlx.txt
on Apple silicon without a remote grounder) into ``<state-dir>/vision/site``,
starts the worker, and keeps it while vision is in use. The grounder's
weights download to the Hugging Face cache on its first grounding call. The
worker stops after ``IDLE_S`` without vision work, which frees the model's
memory.
"""

from __future__ import annotations

import hashlib
import itertools
import json
import logging
import os
import platform
import subprocess
import sys
import threading
import time
from concurrent.futures import Future
from pathlib import Path
from typing import Any

log = logging.getLogger("allternit_driver.vision")

HERE = Path(__file__).resolve().parent
PACKAGE_ROOT = HERE.parent.parent  # The directory holding allternit_driver/.
IDLE_S = 600.0
READY_WAIT_S = 20.0  # How long a read waits for a preparing worker before answering without vision.


class VisionUnavailable(Exception):
    def __init__(self, state: str, detail: str) -> None:
        super().__init__(detail)
        self.state = state


def wants_mlx() -> bool:
    return sys.platform == "darwin" and platform.machine() == "arm64" and not os.environ.get("ALLTERNIT_GROUNDER_URL")


class VisionWorker:
    def __init__(self, state_dir: str | None) -> None:
        base = Path(state_dir) if state_dir else Path.home() / ".cache" / "allternit-driver"
        self.dir = base / "vision"
        self.state = "idle"  # idle | installing | starting | ready | error
        self.detail = ""
        self.error: str | None = None
        self._proc: subprocess.Popen[bytes] | None = None
        self._pending: dict[int, Future] = {}
        self._ids = itertools.count(1)
        self._lock = threading.Lock()
        self._write = threading.Lock()
        self._ready = threading.Event()
        self._last = 0.0
        self._idle: threading.Thread | None = None

    # ---- lifecycle ---------------------------------------------------------------

    def status(self) -> dict[str, Any]:
        return {"state": self.state, "detail": self.detail, "error": self.error, "grounder": "mlx" if wants_mlx() else
                ("remote" if os.environ.get("ALLTERNIT_GROUNDER_URL") else "none")}

    def ensure(self, wait_s: float = READY_WAIT_S) -> None:
        """Start (or keep) the worker; wait up to ``wait_s`` for it."""
        self._last = time.monotonic()
        if self._proc is not None and self._proc.poll() is None and self._ready.is_set():
            return
        with self._lock:
            if self.state not in ("installing", "starting"):
                self.state, self.error = "starting", None
                self._ready.clear()
                threading.Thread(target=self._prepare, name="vision-prepare", daemon=True).start()
        if not self._ready.wait(wait_s):
            raise VisionUnavailable(self.state, self.error or f"vision is {self.state}: {self.detail}")
        if self.state != "ready":
            raise VisionUnavailable(self.state, self.error or "vision worker failed to start")

    def _prepare(self) -> None:
        try:
            sites = [str(self._install("requirements-vision.txt"))]
            if wants_mlx():
                sites.insert(0, str(self._install("requirements-vision-mlx.txt")))
            self.state, self.detail = "starting", "vision worker"
            argv = [sys.executable, "-B", "-s", "-c",
                    "import sys; sys.path.insert(0, %r); from allternit_driver.vision.worker import main; main()" % str(PACKAGE_ROOT)]
            for s in sites:
                argv += ["--site", s]
            argv += ["--models-dir", str(self.dir / "models")]
            self._proc = subprocess.Popen(argv, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
            first = self._proc.stdout.readline() if self._proc.stdout else b""
            if not first or not json.loads(first).get("ready"):
                raise RuntimeError("the vision worker didn't start")
            threading.Thread(target=self._reader, args=(self._proc,), name="vision-reader", daemon=True).start()
            if self._idle is None or not self._idle.is_alive():
                self._idle = threading.Thread(target=self._idle_watch, name="vision-idle", daemon=True)
                self._idle.start()
            self.state, self.detail = "ready", ""
        except Exception as e:  # Reported in status and in the read's vision block; retried on next use.
            log.warning("vision worker unavailable: %s", e)
            self.state, self.error = "error", f"{type(e).__name__}: {e}"[:400]
        finally:
            self._ready.set()

    def _install(self, name: str) -> Path:
        req = HERE / name
        digest = hashlib.sha256(req.read_bytes() + sys.version.encode()).hexdigest()[:12]
        site = self.dir / "site" / f"{req.stem}-{digest}"
        if not (site / ".complete").exists():
            self.state, self.detail = "installing", name
            site.mkdir(parents=True, exist_ok=True)
            cmd = [sys.executable, "-m", "pip", "install", "--disable-pip-version-check", "--no-input",
                   "--upgrade", "--target", str(site), "-r", str(req)]
            out = subprocess.run(cmd, capture_output=True, text=True)
            if out.returncode != 0:
                raise RuntimeError(f"package install failed: {out.stderr.strip()[-400:]}")
            (site / ".complete").write_text(digest)
        return site

    def _reader(self, proc: subprocess.Popen[bytes]) -> None:
        assert proc.stdout is not None
        for line in proc.stdout:
            try:
                msg = json.loads(line)
            except ValueError:
                continue
            fut = self._pending.pop(msg.get("id"), None)
            if fut is not None and not fut.done():
                fut.set_result(msg)
        for fut in list(self._pending.values()):
            if not fut.done():
                fut.set_exception(VisionUnavailable("error", "the vision worker stopped"))
        self._pending.clear()
        if self._proc is proc:
            self._ready.clear()
            self.state = "idle" if self.state == "ready" else self.state

    def _idle_watch(self) -> None:
        while True:
            time.sleep(15)
            if self._proc is not None and self._proc.poll() is None and not self._pending and time.monotonic() - self._last > IDLE_S:
                log.info("vision idle for %.0f s: stopping the worker", IDLE_S)
                self.close()

    def close(self) -> None:
        proc, self._proc = self._proc, None
        self._ready.clear()
        if self.state == "ready":
            self.state = "idle"
        if proc is not None and proc.poll() is None:
            proc.terminate()
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                proc.kill()

    # ---- calls -------------------------------------------------------------------------

    def call(self, op: str, timeout: float = 120.0, **params: Any) -> dict[str, Any]:
        self.ensure()
        proc = self._proc
        if proc is None or proc.stdin is None:
            raise VisionUnavailable("error", "the vision worker isn't running")
        rid = next(self._ids)
        fut: Future = Future()
        self._pending[rid] = fut
        line = (json.dumps({"id": rid, "op": op, **params}) + "\n").encode()
        with self._write:
            proc.stdin.write(line)
            proc.stdin.flush()
        try:
            msg = fut.result(timeout=timeout)
        except TimeoutError:
            self._pending.pop(rid, None)
            raise VisionUnavailable("timeout", f"the vision worker didn't answer {op} in {timeout:.0f}s") from None
        finally:
            self._last = time.monotonic()
        if "error" in msg:
            raise RuntimeError(msg["error"])
        return msg["result"]
