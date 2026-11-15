# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Allternit LLC
"""Outputs: LoRA adapter, merged model (safetensors), GGUF through llama.cpp.

GGUF uses llama.cpp's own convert_hf_to_gguf.py (and llama-quantize for
k-quants) when they are on this computer. When they are not, the job emits a
plain "GGUF export needs llama.cpp" log event and still succeeds with its
other outputs.
"""

from __future__ import annotations

import os
import shutil
import subprocess
import sys
from pathlib import Path
from typing import TYPE_CHECKING, List, Optional

from .spec import GGUF_DIRECT

if TYPE_CHECKING:  # pragma: no cover
    from .events import Emitter

CONVERT_SCRIPT = "convert_hf_to_gguf.py"


def find_llama_cpp() -> dict:
    """Locate llama.cpp's converter and quantizer. Never downloads anything."""
    roots = [os.environ.get("ALLTERNIT_LLAMA_CPP_DIR"), os.environ.get("LLAMA_CPP_DIR"),
             str(Path.home() / "llama.cpp")]
    convert: Optional[str] = shutil.which(CONVERT_SCRIPT)
    quantize: Optional[str] = shutil.which("llama-quantize")
    for root in roots:
        if not root:
            continue
        base = Path(root).expanduser()
        if convert is None and (base / CONVERT_SCRIPT).is_file():
            convert = str(base / CONVERT_SCRIPT)
        if quantize is None:
            for candidate in (base / "build" / "bin" / "llama-quantize", base / "llama-quantize"):
                if candidate.is_file() and os.access(candidate, os.X_OK):
                    quantize = str(candidate)
                    break
    return {"convert": convert, "quantize": quantize}


def base_dtype(base_path: str):
    """The dtype the base model ships in (fp16, bf16 or fp32), so outputs keep
    the base model's size instead of the fp32 copy used for training."""
    import torch
    from transformers import AutoConfig

    config = AutoConfig.from_pretrained(base_path, local_files_only=True)
    saved = getattr(config, "dtype", None) or getattr(config, "torch_dtype", None)
    if isinstance(saved, str):
        saved = getattr(torch, saved, None)
    return saved if saved in (torch.float16, torch.bfloat16, torch.float32) else torch.float32


def save_adapter(model, tokenizer, out: Path, emit: "Emitter") -> Path:
    out.mkdir(parents=True, exist_ok=True)
    model.save_pretrained(str(out), safe_serialization=True)
    tokenizer.save_pretrained(str(out))
    emit.output("adapter", out, f"LoRA adapter written to {out}")
    return out


def merge_adapter(base_path: str, adapter_dir: Path, out: Path, emit: "Emitter") -> Path:
    """Fold the adapter into a full-precision copy of the base and save it."""
    import torch
    from peft import PeftModel
    from transformers import AutoModelForCausalLM, AutoTokenizer

    saved = base_dtype(base_path)
    emit.log(f"merging the adapter into the base model on CPU; saving as {str(saved).replace('torch.', '')}")
    # Merge in fp32 for accuracy, then save in the base model's own dtype.
    base = AutoModelForCausalLM.from_pretrained(base_path, dtype=torch.float32, local_files_only=True)
    merged = PeftModel.from_pretrained(base, str(adapter_dir)).merge_and_unload().to(saved)
    out.mkdir(parents=True, exist_ok=True)
    merged.save_pretrained(str(out), safe_serialization=True)
    AutoTokenizer.from_pretrained(str(adapter_dir)).save_pretrained(str(out))
    emit.output("merged", out, f"merged model written to {out}")
    return out


def save_full(model, tokenizer, base_path: str, out: Path, emit: "Emitter") -> Path:
    out.mkdir(parents=True, exist_ok=True)
    model.to("cpu").to(base_dtype(base_path)).save_pretrained(str(out), safe_serialization=True)
    tokenizer.save_pretrained(str(out))
    emit.output("merged", out, f"fine-tuned model written to {out}")
    return out


def _run(cmd: List[str]) -> Optional[str]:
    """Run a tool; return None on success or the tail of its output."""
    proc = subprocess.run(cmd, capture_output=True, text=True)
    if proc.returncode == 0:
        return None
    tail = (proc.stderr or proc.stdout or "").strip().splitlines()[-5:]
    return " | ".join(tail) or f"exit code {proc.returncode}"


def export_gguf(model_dir: Path, quants: List[str], out: Path, emit: "Emitter") -> List[Path]:
    """Write one GGUF per quant. Missing llama.cpp is a log event, not a crash."""
    tools = find_llama_cpp()
    if tools["convert"] is None:
        emit.log(
            "GGUF export needs llama.cpp: convert_hf_to_gguf.py was not found "
            "(put it on PATH or set ALLTERNIT_LLAMA_CPP_DIR); skipped " + ", ".join(quants)
        )
        return []
    out.mkdir(parents=True, exist_ok=True)
    written: List[Path] = []
    f16: Optional[Path] = None
    for quant in quants:
        target = out / f"model-{quant}.gguf"
        if quant in GGUF_DIRECT:
            err = _run([sys.executable, tools["convert"], str(model_dir), "--outtype", quant, "--outfile", str(target)])
        else:
            if tools["quantize"] is None:
                emit.log(f"GGUF {quant} needs llama.cpp's llama-quantize, which was not found; skipped")
                continue
            if f16 is None:
                f16 = out / "model-f16.gguf"
                err = _run([sys.executable, tools["convert"], str(model_dir), "--outtype", "f16", "--outfile", str(f16)])
                if err:
                    emit.log(f"GGUF export failed while converting to f16: {err}")
                    return written
            err = _run([tools["quantize"], str(f16), str(target), quant.upper()])
        if err:
            emit.log(f"GGUF {quant} export failed: {err}")
            continue
        emit.output("gguf", target, f"GGUF {quant} written to {target}")
        written.append(target)
    if f16 is not None and "f16" not in quants and f16.exists():
        f16.unlink()
    return written
