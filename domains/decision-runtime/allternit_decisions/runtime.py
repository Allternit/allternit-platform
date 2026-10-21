"""First-use preparation: Python packages, model weights, loading.

Nothing heavy ships with the app. On the first decision (or `POST /prepare`)
the sidecar installs its pinned packages into its state directory, downloads
the model into it, and loads it, reporting progress through `/health`. Until
then scoring answers 503 with that status and the caller escalates.
"""

from __future__ import annotations

import hashlib
import importlib
import logging
import os
import platform
import subprocess
import sys
import threading
import time
from pathlib import Path
from typing import Any

log = logging.getLogger("allternit_decisions.runtime")

HERE = Path(__file__).resolve().parent.parent

# Default Qwen3.5-4B: Apache-2.0, no Hugging Face login. Gemma 3 is the switch
# for when an HF login is present (its upstream repo is gated); set HF_TOKEN.
PRESETS: dict[str, dict[str, Any]] = {
    "qwen3.5-4b": {
        "mlx": "mlx-community/Qwen3.5-4B-MLX-4bit",
        "gguf": ("unsloth/Qwen3.5-4B-GGUF", "Qwen3.5-4B-Q4_K_M.gguf"),
        "license": "apache-2.0",
    },
    "gemma-3-4b": {
        "mlx": "mlx-community/gemma-3-4b-it-4bit",
        "gguf": ("unsloth/gemma-3-4b-it-GGUF", "gemma-3-4b-it-Q4_K_M.gguf"),
        "license": "gemma",
    },
}
DEFAULT_PRESET = "qwen3.5-4b"


def default_engine() -> str:
    return "mlx" if sys.platform == "darwin" and platform.machine() == "arm64" else "llamacpp"


class Runtime:
    """Owns the scorer and its preparation state machine."""

    def __init__(self, state_dir: Path, engine: str | None = None, model: str | None = None) -> None:
        self.state_dir = state_dir
        self.engine = engine or os.environ.get("ALLTERNIT_DECISIONS_ENGINE") or default_engine()
        if self.engine not in ("mlx", "llamacpp"):
            raise ValueError(f"unknown engine {self.engine}")
        self.model = model or os.environ.get("ALLTERNIT_DECISIONS_MODEL") or DEFAULT_PRESET
        self.scorer: Any = None
        self._lock = threading.Lock()
        self._thread: threading.Thread | None = None
        self.status = "idle"
        self.progress = 0.0
        self.detail = ""
        self.error: str | None = None
        self.load_s: float | None = None

    # ----------------------------------------------------------- status
    def health(self) -> dict[str, Any]:
        return {
            "status": self.status,
            "progress": round(self.progress, 3),
            "detail": self.detail,
            "error": self.error,
            "engine": self.engine,
            "model": self.model,
            "vision": False,
            "load_s": self.load_s,
        }

    def ensure_started(self) -> None:
        with self._lock:
            if self.status in ("ready",) or (self._thread and self._thread.is_alive()):
                return
            self.error = None
            self._thread = threading.Thread(target=self._prepare, name="prepare", daemon=True)
            self._thread.start()

    def _set(self, status: str, progress: float = 0.0, detail: str = "") -> None:
        self.status, self.progress, self.detail = status, progress, detail
        log.info("%s %.0f%% %s", status, progress * 100, detail)

    # ---------------------------------------------------------- prepare
    def _prepare(self) -> None:
        try:
            self._install()
            path = self._download()
            self._set("loading", 0.0, path)
            t = time.perf_counter()
            if self.engine == "mlx":
                from .engine_mlx import MlxScorer

                scorer = MlxScorer(path)
            else:
                from .engine_llamacpp import LlamaCppScorer

                scorer = LlamaCppScorer(path)
            scorer.score("warm up", ["yes", "no"], "verify")  # compile kernels before the first decision
            self.load_s = round(time.perf_counter() - t, 2)
            self.scorer = scorer
            self._set("ready", 1.0)
        except Exception as e:  # noqa: BLE001 - reported through /health, retried on next use
            log.exception("preparation failed")
            self.error = f"{type(e).__name__}: {e}"
            self._set("error", self.progress, self.detail)

    def _install(self) -> None:
        req = HERE / f"requirements-{self.engine}.txt"
        digest = hashlib.sha256(req.read_bytes() + sys.version.encode()).hexdigest()[:12]
        site = self.state_dir / "site" / f"{self.engine}-{digest}"
        if not (site / ".complete").exists():
            self._set("installing", 0.0, f"Python packages for {self.engine}")
            site.mkdir(parents=True, exist_ok=True)
            cmd = [sys.executable, "-m", "pip", "install", "--disable-pip-version-check", "--no-input",
                   "--upgrade", "--target", str(site), "-r", str(req)]
            out = subprocess.run(cmd, capture_output=True, text=True)
            if out.returncode != 0:
                raise RuntimeError(f"package install failed: {out.stderr.strip()[-600:]}")
            (site / ".complete").write_text(digest)
        if str(site) not in sys.path:
            sys.path.insert(0, str(site))
            importlib.invalidate_caches()

    def _download(self) -> str:
        """Local path of the model, downloading it on first use."""
        if os.path.isdir(self.model) or os.path.isfile(self.model):
            return self.model
        preset = PRESETS.get(self.model)
        if self.engine == "mlx":
            repo, files = (preset["mlx"] if preset else self.model), None
        else:
            if preset:
                repo, files = preset["gguf"][0], [preset["gguf"][1]]
            else:
                repo, _, name = self.model.partition(":")  # "<repo>:<file.gguf>"
                files = [name] if name else None
        from huggingface_hub import HfApi, snapshot_download

        dest = self.state_dir / "models" / repo.replace("/", "--")
        marker = dest / ".complete"
        if not marker.exists():
            info = HfApi().model_info(repo, files_metadata=True)
            wanted = [s for s in info.siblings if files is None or s.rfilename in files]
            total = sum(s.size or 0 for s in wanted) or 1
            done = threading.Event()

            def watch() -> None:
                while not done.wait(0.5):
                    have = sum(f.stat().st_size for f in dest.rglob("*") if f.is_file()) if dest.exists() else 0
                    self._set("downloading", min(0.99, have / total), repo)

            self._set("downloading", 0.0, repo)
            watcher = threading.Thread(target=watch, daemon=True)
            watcher.start()
            try:
                snapshot_download(repo, local_dir=str(dest), allow_patterns=files)
            finally:
                done.set()
            marker.write_text(info.sha or "")
        if self.engine == "llamacpp":
            return str(next(dest.rglob(files[0] if files else "*.gguf")))
        return str(dest)
