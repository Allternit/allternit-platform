# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Allternit LLC
"""What this computer can run. Feeds the engine card and the spec check."""

from __future__ import annotations

import importlib.util
import os
import platform
from typing import Any, Dict

METHODS = ("lora", "qlora", "full", "dpo")
NO_GPU = "no NVIDIA GPU (CUDA) or Apple Silicon GPU (MPS) found"

# Methods that do not work through torch MPS, with the reason. Empty because
# lora, qlora, dpo and full all ran 20 steps on an M1 Pro (torch 2.15,
# bitsandbytes 0.50.2); see README "Apple Silicon".
MPS_UNAVAILABLE: Dict[str, str] = {}
# 4-bit bitsandbytes on MPS was verified with this version; older ones are CUDA-only.
BNB_MPS_MIN = (0, 50)


def _bnb_version() -> tuple:
    from importlib.metadata import PackageNotFoundError, version

    try:
        return tuple(int(p) for p in version("bitsandbytes").split(".")[:2])
    except (PackageNotFoundError, ValueError):
        return ()


def allow_cpu() -> bool:
    """Developer escape hatch: run on CPU (slow). Hosts never set this."""
    return os.environ.get("ALLTERNIT_TUNE_ALLOW_CPU") == "1"


def probe() -> Dict[str, Any]:
    """Device, library versions and the methods this computer cannot run."""
    info: Dict[str, Any] = {
        "os": platform.system().lower(),
        "machine": platform.machine(),
        "device": "none",
        "torch": None,
        "unavailable": {},
    }
    if importlib.util.find_spec("torch") is None:
        info["unavailable"] = {m: "torch is not installed in the engine's environment" for m in METHODS}
        return info
    import torch

    info["torch"] = torch.__version__
    if torch.cuda.is_available():
        info["device"] = "cuda"
        info["gpu"] = torch.cuda.get_device_name(0)
        if importlib.util.find_spec("bitsandbytes") is None:
            info["unavailable"]["qlora"] = "bitsandbytes is not installed"
    elif getattr(torch.backends, "mps", None) is not None and torch.backends.mps.is_available():
        info["device"] = "mps"
        info["unavailable"].update(MPS_UNAVAILABLE)
        bnb = _bnb_version()
        if not bnb:
            info["unavailable"].setdefault("qlora", "bitsandbytes is not installed")
        elif bnb < BNB_MPS_MIN:
            info["unavailable"].setdefault(
                "qlora", "QLoRA on Apple Silicon needs bitsandbytes 0.50 or newer"
            )
    elif allow_cpu():
        info["device"] = "cpu"
        info["unavailable"]["qlora"] = "4-bit QLoRA needs a GPU"
    else:
        info["unavailable"] = {m: NO_GPU for m in METHODS}
    return info
