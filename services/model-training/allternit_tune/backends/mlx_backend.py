# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Allternit LLC
"""mlx backend: LoRA and QLoRA on Apple Silicon with mlx-lm 0.32.2 (MIT).

What mlx-lm 0.32.2 does (mlx_lm/lora.py, mlx_lm/fuse.py, mlx_lm/LORA.md):
- fine-tune types lora, dora and full; no DPO or other preference training.
  Studio runs lora and qlora here; full and DPO go to PyTorch (MPS)
  (engines.json limits.mlx_unsupported_methods).
- QLoRA is LoRA on a quantized model, so the base is first converted to a
  4-bit MLX copy inside the job folder (mlx_lm.convert, q_bits 4).
- fuse folds the adapter into the base; with dequantize it writes plain
  weights. Its own GGUF export is fp16 Llama/Mistral/Mixtral only, so GGUF
  here goes through llama.cpp, the same as the torch backend.

Outputs: `adapter` (mlx-lm adapters.safetensors + adapter_config.json, which
mlx-lm loads; not a PEFT adapter), `merged` (fused, dequantized), `mlx` (fused,
kept 4-bit after QLoRA), `gguf` (llama.cpp from the fused, dequantized model).

Every mlx / mlx-lm import is inside `_mlx_lm()`, so importing this module on
a computer without MLX is safe.
"""

from __future__ import annotations

import copy
import json
import math
import shutil
import types
from pathlib import Path
from typing import TYPE_CHECKING, Any, Dict, List, Optional

from .. import export
from ..data import data_cache_dir, read_rows, shape, shape_rows
from ..hardware import METHOD_NAME, MLX_METHODS
from . import Backend

if TYPE_CHECKING:  # pragma: no cover
    from ..events import Emitter
    from ..spec import TrainPlan

QLORA_BITS = 4
QLORA_GROUP_SIZE = 64


def _mlx_lm() -> types.SimpleNamespace:
    """The mlx-lm functions this backend calls (the only place mlx is imported)."""
    from mlx.utils import tree_unflatten
    from mlx_lm.convert import convert
    from mlx_lm.lora import CONFIG_DEFAULTS, train_model
    from mlx_lm.tuner.datasets import load_dataset
    from mlx_lm.tuner.trainer import TrainingCallback
    from mlx_lm.utils import dequantize_model, load, save

    return types.SimpleNamespace(
        CONFIG_DEFAULTS=CONFIG_DEFAULTS, TrainingCallback=TrainingCallback, convert=convert,
        dequantize_model=dequantize_model, load=load, load_dataset=load_dataset, save=save,
        train_model=train_model, tree_unflatten=tree_unflatten,
    )


def _is_quantized(model_dir: str) -> bool:
    try:
        config = json.loads((Path(model_dir) / "config.json").read_text())
    except (OSError, ValueError):
        return False
    return bool(config.get("quantization"))


def _rows(plan: "TrainPlan") -> List[Dict[str, Any]]:
    """The plan's dataset as rows with the columns mlx-lm reads."""
    path = plan.dataset_path
    if path and path.lower().endswith((".jsonl", ".json")):
        rows, _ = shape_rows(read_rows(path), plan.dataset_format)
        return rows
    from ..data import load

    dataset, _ = shape(load(plan), plan.dataset_format)
    return [dict(row) for row in dataset]


def _schedule(plan: "TrainPlan", iters: int) -> Optional[Dict[str, Any]]:
    args = plan.training_args
    lr = args["learning_rate"]
    warmup = int(args.get("warmup_steps") or 0)
    name = args.get("lr_scheduler_type", "linear")
    if name == "cosine":
        sched = {"name": "cosine_decay", "arguments": [lr, max(iters - warmup, 1)]}
    elif name == "linear":
        sched = {"name": "linear_schedule", "arguments": [lr, 0.0, max(iters - warmup, 1)]}
    elif warmup:
        sched = {"name": "linear_schedule", "arguments": [lr, lr, max(iters - warmup, 1)]}
    else:
        return None
    if warmup:
        sched["warmup"] = warmup
    return sched


def _lora_keys(model: Any, target_modules: Any) -> Optional[List[str]]:
    """mlx-lm `keys` (paths inside one transformer block) for PEFT-style
    target_modules names. None = every linear layer ("all-linear")."""
    if target_modules == "all-linear" or not target_modules:
        return None
    wanted = set(target_modules)
    keys = sorted(p for p, _ in model.layers[0].named_modules() if p.split(".")[-1] in wanted)
    if not keys:
        raise ValueError(f"none of params.target_modules ({', '.join(sorted(wanted))}) is in this model")
    return keys


def train_args(plan: "TrainPlan", defaults: Dict[str, Any], model: Any, model_path: str,
               data_dir: Path, adapter_dir: Path, rows: int) -> types.SimpleNamespace:
    """mlx-lm's lora args for a TrainPlan. mlx-lm counts iterations in
    batches; TrainPlan.max_steps counts optimizer steps (HF semantics)."""
    t = plan.training_args
    batch = max(1, min(int(t["per_device_train_batch_size"]), rows))
    accum = max(1, int(t.get("gradient_accumulation_steps") or 1))
    if t.get("max_steps", -1) and t["max_steps"] > 0:
        iters = int(t["max_steps"]) * accum
    else:
        iters = max(1, math.ceil(float(t["num_train_epochs"]) * rows / batch))
    lora = plan.lora
    params: Dict[str, Any] = {"rank": lora.r, "dropout": lora.lora_dropout, "scale": lora.lora_alpha / lora.r}
    keys = _lora_keys(model, lora.target_modules)
    if keys is not None:
        params["keys"] = keys
    weight_decay = float(t.get("weight_decay") or 0.0)
    args = copy.deepcopy(dict(defaults))
    args.update(
        model=model_path, train=True, test=False, data=str(data_dir), fine_tune_type="lora",
        optimizer="adamw" if weight_decay else "adam", seed=int(t.get("seed", 42)), num_layers=-1,
        batch_size=batch, iters=iters, val_batches=0, learning_rate=t["learning_rate"],
        steps_per_report=max(1, min(10, iters // 20)), steps_per_eval=iters + 1,
        adapter_path=str(adapter_dir), save_every=iters, max_seq_length=plan.max_seq_len,
        grad_checkpoint=bool(t.get("gradient_checkpointing")), grad_accumulation_steps=accum,
        lr_schedule=_schedule(plan, iters), lora_parameters=params, mask_prompt=False,
        report_to=None, project_name=None, resume_adapter_file=None, trust_remote_code=False,
    )
    args.setdefault("optimizer_config", {})
    if weight_decay:
        args["optimizer_config"] = dict(args["optimizer_config"], adamw={"weight_decay": weight_decay})
    return types.SimpleNamespace(**args)


def fuse(mlx: types.SimpleNamespace, model_path: str, adapter_dir: Path, out: Path, dequantize: bool) -> Path:
    """mlx_lm.fuse as a function: base + adapter -> one model folder."""
    model, tokenizer, config = mlx.load(model_path, adapter_path=str(adapter_dir), return_config=True)
    fused = [(n, m.fuse(dequantize=dequantize)) for n, m in model.named_modules() if hasattr(m, "fuse")]
    if fused:
        model.update_modules(mlx.tree_unflatten(fused))
    if dequantize:
        model = mlx.dequantize_model(model)
        config.pop("quantization", None)
        config.pop("quantization_config", None)
    if out.exists():
        shutil.rmtree(out)
    mlx.save(out, model_path, model, tokenizer, config, donate_model=False)
    return out


class MlxBackend(Backend):
    id = "mlx"
    status = "available"

    def run(self, plan: "TrainPlan", emit: "Emitter") -> None:
        job_dir = Path(plan.job_dir)
        try:
            self._run(plan, emit, job_dir)
        finally:
            for temp in (".mlx-data", ".mlx-base-4bit", ".merged-for-gguf"):
                shutil.rmtree(job_dir / temp, ignore_errors=True)
            shutil.rmtree(data_cache_dir(plan), ignore_errors=True)

    def _run(self, plan: "TrainPlan", emit: "Emitter", job_dir: Path) -> None:
        if plan.method not in MLX_METHODS:
            raise RuntimeError(f"MLX does not run {METHOD_NAME[plan.method]}. Pick the automatic or PyTorch backend.")
        mlx = _mlx_lm()
        out_dir = Path(plan.output_dir or plan.job_dir)

        model_path = plan.base_path
        if plan.load_in_4bit and not _is_quantized(model_path):
            model_path = str(job_dir / ".mlx-base-4bit")
            emit.log(f"QLoRA: converting the base model to {QLORA_BITS}-bit MLX")
            mlx.convert(plan.base_path, mlx_path=model_path, quantize=True,
                        q_bits=QLORA_BITS, q_group_size=QLORA_GROUP_SIZE)
        elif _is_quantized(model_path):
            emit.log("the base model is already quantized; LoRA trains on it as QLoRA")

        rows = _rows(plan)
        data_dir = job_dir / ".mlx-data"
        data_dir.mkdir(parents=True, exist_ok=True)
        with open(data_dir / "train.jsonl", "w", encoding="utf-8") as fh:
            for row in rows:
                fh.write(json.dumps(row) + "\n")
        emit.log(f"dataset: {len(rows)} rows, format {plan.dataset_format}")

        emit.log(f"loading base model from {model_path} with MLX")
        model, tokenizer = mlx.load(model_path)
        adapter_dir = out_dir / "adapter"
        args = train_args(plan, mlx.CONFIG_DEFAULTS, model, model_path, data_dir, adapter_dir, len(rows))
        train_set, valid_set, _ = mlx.load_dataset(args, tokenizer)

        iters = args.iters

        class Progress(mlx.TrainingCallback):
            def on_train_loss_report(self, info: Dict[str, Any]) -> None:
                step = int(info.get("iteration", 0))
                emit.progress(step / iters, f"step {step}/{iters} loss {float(info.get('train_loss', 0.0)):.4g}")

        emit.state("running", f"training {plan.method} on this Mac's GPU (MLX)")
        mlx.train_model(args, model, train_set, valid_set, Progress())
        del model
        emit.log(f"training done: {iters} iterations")

        emit.state("exporting")
        emit.output("adapter", adapter_dir, f"MLX LoRA adapter (mlx-lm format) written to {adapter_dir}")
        merged_dir = None
        if plan.want_merged:
            merged_dir = fuse(mlx, model_path, adapter_dir, out_dir / "merged", dequantize=True)
            emit.output("merged", merged_dir, f"merged model written to {merged_dir}")
        if plan.want_mlx:
            mlx_dir = fuse(mlx, model_path, adapter_dir, out_dir / "mlx", dequantize=False)
            emit.output("mlx", mlx_dir, f"MLX model written to {mlx_dir}")
        if plan.gguf:
            if export.find_llama_cpp()["convert"] is None:
                export.export_gguf(out_dir, plan.gguf, out_dir / "gguf", emit)  # logs and skips
                return
            source = merged_dir or fuse(mlx, model_path, adapter_dir, job_dir / ".merged-for-gguf", dequantize=True)
            export.export_gguf(source, plan.gguf, out_dir / "gguf", emit)
