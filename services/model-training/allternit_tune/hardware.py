# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Allternit LLC
"""What this computer can run: the fit for a hardware id, the backend each
method uses, and what the engine's Python environment is missing.

The host detects the hardware and launches the engine with
ALLTERNIT_STUDIO_HARDWARE (one of HARDWARE); the engine does not probe for
itself except as a fallback for a developer run with no host. The rules mirror
services/studio-spec/src/fit.rs for allternit-tune and are asserted against
services/studio-spec/fixtures/fit-table.json.

Importing this module does not import torch or mlx.
"""

from __future__ import annotations

import importlib.util
import os
import platform
import sys
from dataclasses import dataclass
from typing import Any, Dict, Optional, Tuple

METHODS = ("lora", "qlora", "full", "dpo")
HARDWARE = ("apple-silicon", "cuda-linux", "cuda-windows", "rocm-linux", "xpu-linux", "xpu-windows", "cpu")
BACKEND_CHOICES = ("auto", "torch", "mlx")

# engines.json limits.mlx_unsupported_methods is ["dpo", "full"]: mlx-lm
# 0.32.2 has no DPO, and Studio trains full fine-tunes with PyTorch.
MLX_METHODS = ("lora", "qlora")

TORCH_BACKEND = {
    "apple-silicon": "torch-mps",
    "cuda-linux": "torch-cuda",
    "cuda-windows": "torch-cuda",
    "rocm-linux": "torch-rocm",
    "xpu-linux": "torch-xpu",
    "xpu-windows": "torch-xpu",
    "cpu": "torch-cpu",
}
# The torch device a backend trains on. ROCm builds of torch report AMD GPUs
# through torch.cuda (torch.version.hip is set).
TORCH_DEVICE = {
    "torch-mps": "mps",
    "torch-cuda": "cuda",
    "torch-rocm": "cuda",
    "torch-xpu": "xpu",
    "torch-cpu": "cpu",
}
LABEL = {
    "apple-silicon": "a Mac with Apple Silicon",
    "cuda-linux": "an NVIDIA GPU on Linux",
    "cuda-windows": "an NVIDIA GPU on Windows",
    "rocm-linux": "an AMD GPU on Linux",
    "xpu-linux": "an Intel GPU on Linux",
    "xpu-windows": "an Intel GPU on Windows",
    "cpu": "the CPU",
}
OK_REASON = {
    "apple-silicon": "Runs on this Mac's GPU: LoRA and QLoRA with MLX, full fine-tuning and DPO with PyTorch (MPS).",
    "cuda-linux": "Runs on this computer's NVIDIA GPU.",
    "cuda-windows": "Runs on this computer's NVIDIA GPU.",
    "rocm-linux": "Runs on this computer's AMD GPU (ROCm).",
    "xpu-linux": "Runs on this computer's Intel GPU (XPU).",
    "xpu-windows": "Runs on this computer's Intel GPU (XPU).",
    "cpu": "Runs on this computer's CPU.",
}
METHOD_NAME = {"lora": "LoRA", "qlora": "QLoRA", "full": "Full fine-tuning", "dpo": "DPO"}
MLX_OUTPUT_REASON = "MLX files are made only on a Mac with Apple Silicon."
# PyTorch's last Intel Mac (macOS x86_64) wheel is 2.2.2; transformers 5 needs 2.6.
INTEL_MAC_REASON = (
    "PyTorch no longer makes builds for Intel Macs (the last one is 2.2), and "
    "Allternit Tune needs PyTorch 2.6 or newer, so it cannot train on this Mac."
)
CPU_WARNING = "Training on the CPU is slow. Use a small base model; even a small LoRA run can take hours."

# Where bitsandbytes 0.50.2 runs 4-bit QLoRA: CUDA on Linux and Windows, AMD
# ROCm, Intel XPU, Apple CPU + MPS. On `cpu` the fit already refuses QLoRA.
QLORA_HARDWARE = frozenset(HARDWARE) - {"cpu"}
# Before 0.50 bitsandbytes 4-bit was CUDA only.
BNB_MULTI_BACKEND_MIN = (0, 50)

# The requirements file the host installs for each hardware id.
REQUIREMENTS = {hw: f"requirements-{hw}.txt" for hw in HARDWARE}


# -- the fit table (pure) -------------------------------------------------

def method_refusal(hardware: str, method: str) -> Optional[str]:
    """Why `method` cannot run on `hardware` (method_not_supported), or None."""
    if hardware == "cpu" and method != "lora":
        return f"{METHOD_NAME[method]} needs a GPU. Only LoRA runs on the CPU."
    if method == "qlora" and hardware not in QLORA_HARDWARE:  # pragma: no cover - cpu caught above
        return f"QLoRA needs bitsandbytes 4-bit, which does not run on {LABEL[hardware]}."
    return None


def backend_for(hardware: str, method: str, choice: str = "auto") -> Tuple[Optional[str], Optional[str]]:
    """(backend, None) or (None, reason) for backend_not_supported."""
    choice = choice or "auto"
    mlx_here = hardware == "apple-silicon"
    mlx_method = method in MLX_METHODS
    if choice == "mlx":
        if not mlx_here:
            return None, "MLX runs only on a Mac with Apple Silicon."
        if not mlx_method:
            return None, f"MLX does not run {METHOD_NAME[method]} here. Pick the automatic or PyTorch backend."
        return "mlx", None
    if choice == "auto" and mlx_here and mlx_method:
        return "mlx", None
    return TORCH_BACKEND[hardware], None


def output_refusal(hardware: str, output: str) -> Optional[str]:
    """Why this engine cannot make `output` on `hardware` (output_not_supported)."""
    if output == "mlx" and hardware != "apple-silicon":
        return MLX_OUTPUT_REASON
    return None


def fit(hardware: str) -> Dict[str, Any]:
    """The contract's `fit` object for allternit-tune on `hardware`."""
    methods_unavailable = {m: r for m in METHODS if (r := method_refusal(hardware, m))}
    first = next(m for m in METHODS if m not in methods_unavailable)
    outputs_unavailable = {o: r for o in ("adapter", "merged", "gguf", "mlx") if (r := output_refusal(hardware, o))}
    return {
        "ok": True,
        "hardware": hardware,
        "backend": backend_for(hardware, first)[0],
        "reason": OK_REASON[hardware],
        "warning": CPU_WARNING if hardware == "cpu" else None,
        "methods_unavailable": methods_unavailable,
        "outputs_unavailable": outputs_unavailable,
    }


# -- which hardware id --------------------------------------------------------

class HardwareError(ValueError):
    pass


def hardware_id() -> str:
    """ALLTERNIT_STUDIO_HARDWARE from the host, or a best guess for a run
    with no host (a developer starting the server by hand)."""
    value = os.environ.get("ALLTERNIT_STUDIO_HARDWARE", "").strip()
    if value:
        if value not in HARDWARE:
            raise HardwareError(
                f"ALLTERNIT_STUDIO_HARDWARE is '{value}'; expected one of {', '.join(HARDWARE)}"
            )
        return value
    return guess()


def guess() -> str:
    system = platform.system().lower()
    machine = platform.machine().lower()
    if system == "darwin":
        return "apple-silicon" if machine in ("arm64", "aarch64") else "cpu"
    if not _installed("torch"):
        return "cpu"
    import torch

    os_name = "windows" if system == "windows" else "linux"
    if torch.cuda.is_available():
        if getattr(torch.version, "hip", None):
            return "rocm-linux" if os_name == "linux" else "cpu"
        return f"cuda-{os_name}"
    xpu = getattr(torch, "xpu", None)
    if xpu is not None and xpu.is_available():
        return f"xpu-{os_name}"
    return "cpu"


# -- what the engine's environment is missing ---------------------------------

def _installed(name: str) -> bool:
    if name in sys.modules:
        return sys.modules[name] is not None
    try:
        return importlib.util.find_spec(name) is not None
    except (ImportError, ValueError):
        return False


def _version(dist: str) -> Optional[str]:
    from importlib.metadata import PackageNotFoundError, version

    module = sys.modules.get(dist.replace("-", "_"))
    if module is not None and getattr(module, "__version__", None):
        return str(module.__version__)
    try:
        return version(dist)
    except PackageNotFoundError:
        return None


def _version_tuple(text: Optional[str]) -> tuple:
    out = []
    for part in (text or "").split(".")[:2]:
        digits = "".join(c for c in part if c.isdigit())
        if not digits:
            break
        out.append(int(digits))
    return tuple(out)


def torch_device_gap(backend: str, hardware: str) -> Optional[str]:
    """None when torch in this environment can train on `backend`'s device,
    else the reason. Imports torch."""
    if platform.system() == "Darwin" and platform.machine().lower() == "x86_64":
        return INTEL_MAC_REASON
    fix = f"Reinstall the engine with {REQUIREMENTS[hardware]}."
    if not _installed("torch"):
        return f"PyTorch is not installed in the engine's environment. {fix}"
    import torch

    device = TORCH_DEVICE[backend]
    hip = getattr(getattr(torch, "version", None), "hip", None)
    if device == "mps":
        mps = getattr(getattr(torch, "backends", None), "mps", None)
        ok = mps is not None and mps.is_available()
    elif device == "xpu":
        xpu = getattr(torch, "xpu", None)
        ok = xpu is not None and xpu.is_available()
    elif device == "cuda":
        ok = torch.cuda.is_available() and (bool(hip) if backend == "torch-rocm" else not hip)
    else:
        ok = True
    if ok:
        return None
    name = " ".join(p for p in ("PyTorch", str(getattr(torch, "__version__", "") or "")) if p)
    return f"{name} in the engine's environment cannot use {LABEL[hardware]}. {fix}"


def bnb_gap(hardware: str, backend: str) -> Optional[str]:
    """Why QLoRA through bitsandbytes cannot run here, or None."""
    version = _version("bitsandbytes") if _installed("bitsandbytes") else None
    if version is None:
        return f"QLoRA needs bitsandbytes, which is not installed. Reinstall the engine with {REQUIREMENTS[hardware]}."
    if backend != "torch-cuda" and _version_tuple(version) < BNB_MULTI_BACKEND_MIN:
        return f"QLoRA on {LABEL[hardware]} needs bitsandbytes 0.50 or newer (found {version})."
    return None


def mlx_gap() -> Optional[str]:
    if _installed("mlx_lm"):
        return None
    return f"mlx-lm is not installed in the engine's environment. Reinstall the engine with {REQUIREMENTS['apple-silicon']}."


@dataclass
class Host:
    """The computer a job runs on. `check_env` off means the fit table only
    (tests, and the contract cells); on means the engine's own environment is
    checked too (torch can see the GPU, bitsandbytes, mlx-lm)."""

    hardware: str
    check_env: bool = False

    @classmethod
    def detect(cls) -> "Host":
        return cls(hardware_id(), check_env=True)

    def env_gap(self, backend: str, method: str) -> Optional[Tuple[str, str]]:
        """(code, reason) when this environment cannot run `method` on `backend`."""
        if not self.check_env:
            return None
        if backend == "mlx":
            gap = mlx_gap()
            return ("backend_not_supported", gap) if gap else None
        gap = torch_device_gap(backend, self.hardware)
        if gap is None and method == "qlora":
            gap = bnb_gap(self.hardware, backend)
        return ("method_not_supported", gap) if gap else None

    def output_gap(self, output: str) -> Optional[str]:
        if not self.check_env or output != "mlx":
            return None
        return mlx_gap()

    def card(self) -> Dict[str, Any]:
        """`fit` plus what this environment is missing, for GET /v1/engine."""
        out = fit(self.hardware)
        env_reasons = []
        for method in METHODS:
            if method in out["methods_unavailable"]:
                continue
            backend = backend_for(self.hardware, method)[0]
            gap = self.env_gap(backend, method)
            if gap:
                out["methods_unavailable"][method] = gap[1]
                env_reasons.append(gap[1])
        if "mlx" not in out["outputs_unavailable"]:
            gap = self.output_gap("mlx")
            if gap:
                out["outputs_unavailable"]["mlx"] = gap
        if len(out["methods_unavailable"]) == len(METHODS):
            out["ok"] = False
            out["backend"] = None
            # What blocks this computer is the environment, not the fit table.
            out["reason"] = env_reasons[0] if env_reasons else next(iter(out["methods_unavailable"].values()))
        return out


def probe() -> Dict[str, Any]:
    """Hardware id, fit and library versions for the engine card and the
    server's ready line. Imports torch when it is installed."""
    host = Host.detect()
    torch_version = _version("torch") if _installed("torch") else None
    mlx_version = _version("mlx-lm") if host.hardware == "apple-silicon" and _installed("mlx_lm") else None
    return {
        "hardware": host.hardware,
        "host": host,
        "fit": host.card(),
        "torch": torch_version,
        "mlx_lm": mlx_version,
        "os": platform.system().lower(),
        "machine": platform.machine(),
    }
