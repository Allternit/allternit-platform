# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Allternit LLC
"""Training backends.

A backend turns a TrainPlan into files. Each one says which methods it can
run on which device, so the engine card and the spec check stay honest.

- torch: TRL + PEFT on CUDA or Apple Silicon (MPS). Built.
- mlx: Apple MLX. Waiting on a product decision (Decision 3 of the Studio
  Phase 3 plan); registered as `pending_decision` so the card shows it and a
  later change only has to fill in `MlxBackend`.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, Dict, List, Optional

if TYPE_CHECKING:  # pragma: no cover
    from ..events import Emitter
    from ..spec import TrainPlan


class Backend:
    """Interface every backend implements."""

    id: str = ""
    #: planned | available | pending_decision (studio-api EngineInfo.backends)
    status: str = "planned"

    def unavailable_methods(self, device: str) -> Dict[str, str]:
        """Methods this backend cannot run on `device`, with the reason."""
        raise NotImplementedError

    def run(self, plan: "TrainPlan", emit: "Emitter") -> None:
        """Train, then write the outputs the plan asks for. Emits state
        `running` before training and `exporting` before writing outputs, and
        one `output` event per file or folder written. Raises on failure."""
        raise NotImplementedError


class MlxBackend(Backend):
    id = "mlx"
    status = "pending_decision"

    def unavailable_methods(self, device: str) -> Dict[str, str]:
        reason = "the MLX backend is waiting on a product decision"
        return {m: reason for m in ("lora", "qlora", "full", "dpo")}

    def run(self, plan: "TrainPlan", emit: "Emitter") -> None:
        raise RuntimeError("the MLX backend is waiting on a product decision")


def all_backends() -> List[Backend]:
    from .torch_backend import TorchBackend

    return [TorchBackend(), MlxBackend()]


def select_backend(device: str, name: Optional[str] = None) -> Backend:
    """The backend that runs jobs on `device`. Today that is always torch."""
    for backend in all_backends():
        if (name is None and backend.id == "torch") or backend.id == name:
            return backend
    raise ValueError(f"unknown backend: {name}")
