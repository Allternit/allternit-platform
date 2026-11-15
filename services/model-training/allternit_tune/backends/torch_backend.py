# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Allternit LLC
"""torch backend: TRL + PEFT on CUDA or Apple Silicon (MPS).

- lora: PEFT LoRA adapters, TRL SFTTrainer.
- qlora: base loaded 4-bit through bitsandbytes, then LoRA.
- dpo: TRL DPOTrainer with LoRA adapters (the reference model is the base
  with adapters turned off, so no second copy is loaded).
- full: every weight trains, TRL SFTTrainer, no adapter.
"""

from __future__ import annotations

import gc
import shutil
from pathlib import Path
from typing import TYPE_CHECKING, Any, Dict

from .. import export
from ..data import DataError, data_cache_dir, load, shape
from ..hardware import probe
from . import Backend

if TYPE_CHECKING:  # pragma: no cover
    from ..events import Emitter
    from ..spec import TrainPlan


def _progress_callback(emit: "Emitter"):
    from transformers import TrainerCallback

    class Progress(TrainerCallback):
        def on_log(self, args, state, control, logs=None, **kwargs):  # noqa: ANN001
            if not logs or state.max_steps <= 0 or "train_runtime" in logs:
                return
            parts = [f"step {state.global_step}/{state.max_steps}"]
            for key in ("loss", "rewards/accuracies", "grad_norm", "learning_rate"):
                if key in logs:
                    parts.append(f"{key.split('/')[-1]} {logs[key]:.4g}")
            emit.progress(state.global_step / state.max_steps, " ".join(parts))

    return Progress()


def _free_memory() -> None:
    import torch

    gc.collect()
    if torch.cuda.is_available():
        torch.cuda.empty_cache()
    elif torch.backends.mps.is_available():
        torch.mps.empty_cache()


class TorchBackend(Backend):
    id = "torch"
    status = "available"

    def unavailable_methods(self, device: str) -> Dict[str, str]:
        return dict(probe()["unavailable"])

    def _load(self, plan: "TrainPlan", device: str, emit: "Emitter"):
        import torch
        from transformers import AutoModelForCausalLM, AutoTokenizer

        tokenizer = AutoTokenizer.from_pretrained(plan.base_path, local_files_only=True)
        if tokenizer.pad_token is None:
            tokenizer.pad_token = tokenizer.eos_token

        kwargs: Dict[str, Any] = {"local_files_only": True}
        if device == "cuda":
            dtype = torch.bfloat16 if torch.cuda.is_bf16_supported() else torch.float16
        else:
            # MPS mixed precision training is unreliable; small models fit in fp32.
            dtype = torch.float32
        kwargs["dtype"] = dtype
        if plan.load_in_4bit:
            from transformers import BitsAndBytesConfig

            kwargs["quantization_config"] = BitsAndBytesConfig(
                load_in_4bit=True,
                bnb_4bit_quant_type="nf4",
                bnb_4bit_use_double_quant=True,
                bnb_4bit_compute_dtype=dtype,
            )
            kwargs["device_map"] = {"": 0 if device == "cuda" else device}
        emit.log(f"loading base model from {plan.base_path} ({str(dtype).replace('torch.', '')}"
                 f"{', 4-bit' if plan.load_in_4bit else ''}) on {device}")
        model = AutoModelForCausalLM.from_pretrained(plan.base_path, **kwargs)
        if plan.load_in_4bit:
            from peft import prepare_model_for_kbit_training

            model = prepare_model_for_kbit_training(
                model, use_gradient_checkpointing=plan.training_args.get("gradient_checkpointing", False)
            )
        return model, tokenizer, dtype

    def run(self, plan: "TrainPlan", emit: "Emitter") -> None:
        try:
            self._run(plan, emit)
        finally:
            shutil.rmtree(data_cache_dir(plan), ignore_errors=True)

    def _run(self, plan: "TrainPlan", emit: "Emitter") -> None:
        import torch
        from peft import LoraConfig

        device = probe()["device"]
        if device == "none":
            raise RuntimeError("no NVIDIA GPU (CUDA) or Apple Silicon GPU (MPS) found")
        emit.log(f"torch {torch.__version__} on {device}; method {plan.method}")
        job_dir = Path(plan.job_dir)

        model, tokenizer, dtype = self._load(plan, device, emit)

        raw = load(plan)
        dataset, fmt = shape(raw, plan.dataset_format)
        if fmt == "chat" and not getattr(tokenizer, "chat_template", None):
            raise DataError("this base model has no chat template; use format text, alpaca or completion")
        emit.log(f"dataset: {len(dataset)} rows, format {fmt}")

        peft_config = None
        if plan.lora is not None:
            peft_config = LoraConfig(
                r=plan.lora.r,
                lora_alpha=plan.lora.lora_alpha,
                lora_dropout=plan.lora.lora_dropout,
                target_modules=plan.lora.target_modules,
                task_type="CAUSAL_LM",
            )

        common: Dict[str, Any] = dict(plan.training_args)
        common.update(
            output_dir=str(job_dir / "checkpoints"),
            save_strategy="no",
            report_to="none",
            disable_tqdm=True,
            bf16=device == "cuda" and dtype == torch.bfloat16,
            fp16=device == "cuda" and dtype == torch.float16,
            use_cpu=device == "cpu",
        )
        callbacks = [_progress_callback(emit)]
        if plan.method == "dpo":
            from trl import DPOConfig, DPOTrainer

            trainer = DPOTrainer(
                model=model, ref_model=None, args=DPOConfig(**common), train_dataset=dataset,
                processing_class=tokenizer, peft_config=peft_config, callbacks=callbacks,
            )
        else:
            from trl import SFTConfig, SFTTrainer

            common.pop("beta", None)
            trainer = SFTTrainer(
                model=model, args=SFTConfig(**common), train_dataset=dataset,
                processing_class=tokenizer, peft_config=peft_config, callbacks=callbacks,
            )

        # With tqdm off, Trainer prints every log dict to stdout; the progress
        # callback already reports them as events.
        from transformers.trainer_callback import PrinterCallback

        trainer.remove_callback(PrinterCallback)
        emit.state("running", f"training {plan.method} on {device}")
        result = trainer.train()
        emit.log(f"training done: {result.global_step} steps, final loss {result.training_loss:.4f}")

        emit.state("exporting")
        if plan.method == "full":
            merged_dir = export.save_full(trainer.model, tokenizer, plan.base_path, job_dir / "merged", emit)
            del trainer, model
            _free_memory()
        else:
            adapter_dir = export.save_adapter(trainer.model, tokenizer, job_dir / "adapter", emit)
            del trainer, model
            _free_memory()
            merged_dir = None
            if plan.want_merged:
                merged_dir = export.merge_adapter(plan.base_path, adapter_dir, job_dir / "merged", emit)
        if plan.gguf:
            temp = None
            if merged_dir is None:
                tools = export.find_llama_cpp()
                if tools["convert"] is None:
                    export.export_gguf(job_dir, plan.gguf, job_dir / "gguf", emit)  # logs and skips
                    return
                temp = job_dir / ".merged-for-gguf"
                merged_dir = export.merge_adapter(plan.base_path, adapter_dir, temp, _Quiet(emit))
            export.export_gguf(merged_dir, plan.gguf, job_dir / "gguf", emit)
            if temp is not None:
                shutil.rmtree(temp, ignore_errors=True)


class _Quiet:
    """Emitter wrapper that drops `output` events (for a temporary merge)."""

    def __init__(self, emit: "Emitter") -> None:
        self._emit = emit

    def output(self, *args: Any, **kwargs: Any) -> None:
        pass

    def __getattr__(self, name: str) -> Any:
        return getattr(self._emit, name)
