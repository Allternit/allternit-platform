# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Allternit LLC
"""Training backends.

A backend turns a TrainPlan into files. Which backend a job uses is decided
when the spec is checked (hardware.backend_for, the contract's rule) and
stored in `TrainPlan.backend`:

- torch (torch-cuda, torch-rocm, torch-xpu, torch-mps, torch-cpu): TRL + PEFT.
- mlx: Apple MLX through mlx-lm, LoRA and QLoRA on a Mac. Imported only when a
  job runs on it, so other computers never import mlx.
"""

from __future__ import annotations

from typing import TYPE_CHECKING, List

if TYPE_CHECKING:  # pragma: no cover
    from ..events import Emitter
    from ..spec import TrainPlan


class Backend:
    """Interface every backend implements."""

    id: str = ""
    #: planned | available | pending_decision (studio-api EngineInfo.backends)
    status: str = "planned"

    def run(self, plan: "TrainPlan", emit: "Emitter") -> None:
        """Train, then write the outputs the plan asks for. Emits state
        `running` before training and `exporting` before writing outputs, and
        one `output` event per file or folder written. Raises on failure."""
        raise NotImplementedError


def all_backends() -> List[Backend]:
    from .mlx_backend import MlxBackend
    from .torch_backend import TorchBackend

    return [TorchBackend(), MlxBackend()]


def select_backend(backend: str) -> Backend:
    """The backend for a plan's `backend` id (mlx or torch-<device>)."""
    if backend == "mlx":
        from .mlx_backend import MlxBackend

        return MlxBackend()
    if backend.startswith("torch-"):
        from .torch_backend import TorchBackend

        return TorchBackend()
    raise ValueError(f"unknown backend: {backend}")
