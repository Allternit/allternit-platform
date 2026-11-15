# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Allternit LLC
"""Jobs: state machine, event log, and the worker process behind each job.

States are forward-only (studio-spec src/job.rs): queued -> preparing ->
running -> exporting -> succeeded; any unfinished state may go to failed or
cancelled; finished states never move. Event seq starts at 1 and goes up by 1.
One job runs at a time.
"""

from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import threading
import uuid
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable, Dict, List, Optional

from .events import decode
from .spec import TrainPlan, build_plan

TERMINAL = {"succeeded", "failed", "cancelled"}
_FORWARD = {("queued", "preparing"), ("preparing", "running"), ("running", "exporting"), ("exporting", "succeeded")}
LOG_LINE_LIMIT = 2000


def can_transition(src: str, dst: str) -> bool:
    if src in TERMINAL:
        return False
    return (src, dst) in _FORWARD or dst in ("failed", "cancelled")


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="milliseconds").replace("+00:00", "Z")


class JobRunning(Exception):
    pass


class AlreadyFinished(Exception):
    pass


WorkerCommand = Callable[[Path], List[str]]


def default_worker_command(plan_path: Path) -> List[str]:
    return [sys.executable, "-m", "allternit_tune.worker", str(plan_path)]


class Job:
    def __init__(self, job_id: str, spec: Dict[str, Any], plan: TrainPlan, job_dir: Path) -> None:
        self.id = job_id
        self.spec = spec
        self.plan = plan
        self.dir = job_dir
        self.state = "queued"
        self.created_at = now_iso()
        self.updated_at: Optional[str] = None
        self.error: Optional[str] = None
        self.events: List[Dict[str, Any]] = []
        self.outputs: List[Dict[str, Any]] = []
        self.process: Optional[subprocess.Popen] = None
        self.lock = threading.RLock()
        self.done = threading.Event()

    # -- events -----------------------------------------------------------
    def _event(self, kind: str, message: str, progress: Optional[float] = None) -> Dict[str, Any]:
        event: Dict[str, Any] = {"seq": len(self.events) + 1, "at": now_iso(), "kind": kind, "message": message}
        if progress is not None:
            event["progress"] = progress
        self.events.append(event)
        self.updated_at = event["at"]
        try:
            with open(self.dir / "events.jsonl", "a", encoding="utf-8") as fh:
                fh.write(json.dumps(event) + "\n")
        except OSError:
            pass
        return event

    def add_event(self, kind: str, message: str, progress: Optional[float] = None) -> None:
        with self.lock:
            self._event(kind, message, progress)

    def set_state(self, state: str, message: Optional[str] = None) -> bool:
        """Move to `state` if allowed; record a state event. False if refused."""
        with self.lock:
            if not can_transition(self.state, state):
                return False
            self.state = state
            self._event("state", message or state)
            if state in TERMINAL:
                self.done.set()
            return True

    # -- views ------------------------------------------------------------
    def to_json(self) -> Dict[str, Any]:
        with self.lock:
            return {
                "id": self.id,
                "state": self.state,
                "spec": self.spec,
                "created_at": self.created_at,
                "updated_at": self.updated_at,
                "error": self.error,
                "last_seq": len(self.events),
                "outputs": list(self.outputs),
            }

    def events_after(self, after: int) -> Dict[str, Any]:
        with self.lock:
            return {"state": self.state, "events": [e for e in self.events if e["seq"] > after]}


class JobManager:
    """Owns every job this engine process has seen and runs one at a time."""

    def __init__(
        self,
        data_dir: Path,
        unavailable_methods: Optional[Dict[str, str]] = None,
        worker_command: WorkerCommand = default_worker_command,
        cancel_grace_s: float = 10.0,
    ) -> None:
        self.data_dir = Path(data_dir)
        self.unavailable_methods = unavailable_methods or {}
        self.worker_command = worker_command
        self.cancel_grace_s = cancel_grace_s
        self.jobs: Dict[str, Job] = {}
        self._lock = threading.Lock()

    def get(self, job_id: str) -> Optional[Job]:
        return self.jobs.get(job_id)

    def submit(self, spec: Dict[str, Any]) -> Job:
        """Check the spec, create the job (queued) and start it. Raises
        SpecError (422) or JobRunning (409)."""
        plan = build_plan(spec, self.unavailable_methods)
        with self._lock:
            if any(j.state not in TERMINAL for j in self.jobs.values()):
                raise JobRunning("a job is already running on this engine")
            job_id = "job_" + uuid.uuid4().hex[:12]
            job_dir = self.data_dir / "jobs" / job_id
            job_dir.mkdir(parents=True, exist_ok=True)
            plan.job_dir = str(job_dir)
            job = Job(job_id, spec, plan, job_dir)
            job.add_event("state", "queued")
            self.jobs[job_id] = job
        threading.Thread(target=self._run, args=(job,), name=f"job-{job_id}", daemon=True).start()
        return job

    def cancel(self, job_id: str) -> Job:
        job = self.jobs[job_id]
        with job.lock:
            if job.state in TERMINAL:
                raise AlreadyFinished(f"job {job_id} already {job.state}")
            job.set_state("cancelled", "cancelled by request")
            proc = job.process
        if proc is not None:
            self._stop(proc)
        return job

    # -- internals --------------------------------------------------------
    def _stop(self, proc: subprocess.Popen) -> None:
        """SIGTERM the worker's process group, then SIGKILL after the grace."""
        if proc.poll() is not None:
            return
        try:
            os.killpg(proc.pid, signal.SIGTERM)
        except (ProcessLookupError, PermissionError):
            proc.terminate()
        try:
            proc.wait(timeout=self.cancel_grace_s)
        except subprocess.TimeoutExpired:
            try:
                os.killpg(proc.pid, signal.SIGKILL)
            except (ProcessLookupError, PermissionError):
                proc.kill()
            proc.wait()

    def _fail(self, job: Job, message: str) -> None:
        with job.lock:
            if job.state in TERMINAL:
                return
            job.error = message
            job._event("error", message)
            job.set_state("failed", f"failed: {message}")

    def _handle(self, job: Job, event: Dict[str, Any], last_error: List[str]) -> None:
        kind = event.get("kind")
        message = str(event.get("message", ""))
        if kind == "state":
            target = event.get("state")
            if target in ("running", "exporting"):
                if not job.set_state(target, message or target) and job.state not in TERMINAL:
                    job.add_event("log", f"ignored state change {job.state} -> {target}")
            return
        if kind == "output" and isinstance(event.get("output"), dict):
            out = event["output"]
            if out.get("kind") in ("adapter", "merged", "gguf", "mlx") and out.get("path"):
                with job.lock:
                    job.outputs.append({k: out[k] for k in ("kind", "path", "bytes") if k in out})
                    job._event("output", message or f"{out['kind']}: {out['path']}")
            return
        if kind == "progress":
            progress = event.get("progress")
            job.add_event("progress", message, float(progress) if isinstance(progress, (int, float)) else None)
            return
        if kind == "error":
            last_error.append(message)
            return  # recorded once by _fail when the worker exits
        job.add_event("log", message[:LOG_LINE_LIMIT])

    def _run(self, job: Job) -> None:
        plan_path = job.dir / "plan.json"
        plan_path.write_text(json.dumps(job.plan.to_dict(), indent=2), encoding="utf-8")
        with job.lock:
            if not job.set_state("preparing", "starting the worker"):
                return  # cancelled while queued
            stderr = open(job.dir / "worker.log", "w", encoding="utf-8")
            env = dict(os.environ)
            pkg_root = str(Path(__file__).resolve().parent.parent)
            env["PYTHONPATH"] = pkg_root + (os.pathsep + env["PYTHONPATH"] if env.get("PYTHONPATH") else "")
            env["PYTHONUNBUFFERED"] = "1"
            try:
                job.process = subprocess.Popen(
                    self.worker_command(plan_path), stdout=subprocess.PIPE, stderr=stderr,
                    text=True, bufsize=1, env=env, start_new_session=True,
                )
            except OSError as exc:
                stderr.close()
                self._fail(job, f"could not start the worker: {exc}")
                return
        proc = job.process
        timer = None
        if job.plan.max_hours:
            def _timeout() -> None:
                if proc.poll() is None and job.state not in TERMINAL:
                    self._fail(job, f"max_hours {job.plan.max_hours} reached")
                    self._stop(proc)
            timer = threading.Timer(job.plan.max_hours * 3600, _timeout)
            timer.daemon = True
            timer.start()

        last_error: List[str] = []
        assert proc.stdout is not None
        for line in proc.stdout:
            line = line.rstrip("\n")
            if not line:
                continue
            if job.state in TERMINAL:
                continue  # drain after cancel/timeout without recording
            event = decode(line)
            if event is None:
                job.add_event("log", line[:LOG_LINE_LIMIT])
            else:
                self._handle(job, event, last_error)
        code = proc.wait()
        stderr.close()
        if timer is not None:
            timer.cancel()
        if job.state in TERMINAL:
            return
        if code == 0:
            with job.lock:
                if job.state == "running":
                    job.set_state("exporting")
                if not job.set_state("succeeded", "finished"):
                    self._fail(job, f"worker exited while the job was {job.state}")
            return
        message = last_error[-1] if last_error else self._stderr_tail(job) or f"worker exited with code {code}"
        self._fail(job, message)

    @staticmethod
    def _stderr_tail(job: Job) -> str:
        try:
            lines = (job.dir / "worker.log").read_text(encoding="utf-8", errors="replace").strip().splitlines()
        except OSError:
            return ""
        return " | ".join(lines[-3:])

    def wait(self, job_id: str, timeout: Optional[float] = None) -> bool:
        return self.jobs[job_id].done.wait(timeout)
