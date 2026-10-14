"""Native decision models (Liquid d1-3B) through transformers.

A decision model is post-trained to answer typed questions (choice / score /
noul) in one forward pass with calibrated probabilities and no generation.
d1-3B ships its own code (`trust_remote_code`, pinned revision in
runtime.PRESETS) exposing `system_one(state, questions)`, the Jev/SystemOne
schema. This engine maps `/v1/score` onto one `choice` question and also
passes whole SystemOne requests through (`POST /v1/systemone`), so
allternit-api's `open` backend can use the sidecar directly.
"""

from __future__ import annotations

import time
from typing import Any

import torch
from transformers import AutoModel


def _device() -> str:
    if torch.cuda.is_available():
        return "cuda"
    if getattr(torch.backends, "mps", None) and torch.backends.mps.is_available():
        return "mps"
    return "cpu"


class SystemOneScorer:
    engine = "systemone"
    vision = False

    def __init__(self, model_path: str) -> None:
        self.device = _device()
        dtype = torch.float32 if self.device == "cpu" else torch.bfloat16
        self.model = AutoModel.from_pretrained(model_path, trust_remote_code=True, dtype=dtype).to(self.device).eval()
        self._lock = __import__("threading").Lock()

    def system_one(self, state: Any, questions: dict[str, Any]) -> dict[str, Any]:
        with self._lock, torch.inference_mode():
            return self.model.system_one(state, questions)

    def score(self, context: str, options: list[str], kind: str = "choice", question: str | None = None) -> dict[str, Any]:
        names = [f"o{i}" for i in range(len(options))]
        q = {
            "type": "choice",
            "instructions": question or f"Decision kind: {kind}. Choose the best option.",
            "criteria": dict(zip(names, options)),
        }
        t0 = time.perf_counter()
        out = self.system_one(context, {"decision": q})
        t1 = time.perf_counter()
        probs_by_name = out["answers"]["decision"]["probabilities"]
        probs = [float(probs_by_name.get(n, 0.0)) for n in names]
        z = sum(probs) or 1.0
        return {
            "probs": [p / z for p in probs],
            "timing": {
                "total_ms": round((t1 - t0) * 1000, 2),
                "prompt_tokens": out.get("usage", {}).get("input_tokens"),
            },
        }
