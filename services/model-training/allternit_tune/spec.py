# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Allternit LLC
"""Job spec v1 checks and the mapping from a spec to a training plan.

The shape and refusal codes mirror services/studio-spec/src/spec.rs. On top of
those, this engine refuses what it cannot do: a base model that is not a local
folder (the host fetches and verifies weights; this engine never downloads
them), unknown params, MLX output, and methods this computer cannot run.

Pure Python: importing this module does not import torch.
"""

from __future__ import annotations

import os
import re
from dataclasses import asdict, dataclass, field
from pathlib import Path
from typing import Any, Dict, List, Optional

from . import ENGINE_ID

SPEC_VERSION = 1
METHODS = ("lora", "qlora", "full", "dpo")
TOP_LEVEL_KEYS = {
    "spec_version", "engine", "engine_version", "base", "dataset", "method",
    "params", "resources", "outputs", "eval", "publish",
}
PLACES = {"local", "paired", "compute", "rented"}
DATASET_FORMATS = {"jsonl", "chat", "alpaca", "text", "completion", "dpo"}
DATASET_SUFFIXES = {".jsonl", ".json", ".parquet"}
MAX_SEQ_LEN = 8192

# Quant names llama.cpp's converter writes directly, and the ones that need
# llama-quantize afterwards.
GGUF_DIRECT = {"f32", "f16", "bf16", "q8_0"}
GGUF_QUANTIZE = {
    "q2_k", "q3_k_s", "q3_k_m", "q3_k_l", "q4_0", "q4_1", "q4_k_s", "q4_k_m",
    "q5_0", "q5_1", "q5_k_s", "q5_k_m", "q6_k", "iq4_xs", "iq4_nl",
}

_REPO_ID = re.compile(r"^[A-Za-z0-9][\w.-]*(/[\w.-]+)?$")


class SpecError(Exception):
    """A refused spec. `code` is one of the studio-api createJob codes."""

    def __init__(self, code: str, message: str) -> None:
        super().__init__(f"{code}: {message}")
        self.code = code
        self.message = message


# name -> (type check, default per method or a single default, description)
_PARAM_TYPES: Dict[str, type] = {
    "rank": int,
    "alpha": int,
    "dropout": float,
    "target_modules": list,
    "lr": float,
    "epochs": float,
    "max_steps": int,
    "batch_size": int,
    "grad_accum": int,
    "warmup_steps": int,
    "weight_decay": float,
    "max_seq_len": int,
    "seed": int,
    "beta": float,
    "lr_scheduler": str,
    "gradient_checkpointing": bool,
}
KNOWN_PARAMS = tuple(sorted(_PARAM_TYPES))

_DEFAULT_LR = {"lora": 2e-4, "qlora": 2e-4, "full": 2e-5, "dpo": 5e-5}


@dataclass
class LoraSettings:
    r: int
    lora_alpha: int
    lora_dropout: float
    target_modules: Any  # "all-linear" or a list of module names


@dataclass
class TrainPlan:
    """Everything a backend needs. Serialised to JSON for the worker process."""

    method: str
    base_path: str
    dataset_path: Optional[str]
    dataset_repo: Optional[str]
    dataset_revision: Optional[str]
    dataset_format: str
    lora: Optional[LoraSettings]
    load_in_4bit: bool
    training_args: Dict[str, Any]
    max_seq_len: int
    beta: Optional[float]
    want_adapter: bool
    want_merged: bool
    gguf: List[str]
    max_hours: Optional[float]
    notes: List[str] = field(default_factory=list)
    job_dir: str = ""

    def to_dict(self) -> Dict[str, Any]:
        return asdict(self)

    @classmethod
    def from_dict(cls, data: Dict[str, Any]) -> "TrainPlan":
        data = dict(data)
        if data.get("lora") is not None:
            data["lora"] = LoraSettings(**data["lora"])
        return cls(**data)


def _expand(path: str) -> Path:
    return Path(os.path.expandvars(os.path.expanduser(path)))


def _looks_like_path(value: str) -> bool:
    return value.startswith(("/", "~", "./", "../", "$")) or value in (".", "..")


def resolve_base(base: Any) -> Path:
    """Return the local base-model folder, or refuse.

    The host fetches the model with fetch_verified, checks its sha256, and
    sends the local folder in base.repo (base.path is accepted too).
    """
    if not isinstance(base, dict):
        raise SpecError("invalid_spec", "base must be an object")
    value = base.get("path") or base.get("repo")
    if not isinstance(value, str) or not value.strip():
        raise SpecError("invalid_spec", "base.repo is required")
    value = value.strip()
    if not _looks_like_path(value):
        raise SpecError(
            "invalid_spec",
            f"base.repo '{value}' is a repo id. allternit-tune never downloads "
            "weights: the host fetches and verifies the model, then sends its "
            "local folder path in base.repo.",
        )
    folder = _expand(value)
    if not folder.is_dir():
        raise SpecError("invalid_spec", f"base model folder not found: {folder}")
    if not (folder / "config.json").is_file():
        raise SpecError("invalid_spec", f"base model folder has no config.json: {folder}")
    return folder.resolve()


def _check_dataset(dataset: Any, method: str) -> Dict[str, Optional[str]]:
    if not isinstance(dataset, dict):
        raise SpecError("invalid_dataset", "dataset must be an object")
    repo, path = dataset.get("repo"), dataset.get("path")
    if bool(repo) == bool(path):
        raise SpecError("invalid_dataset", "dataset needs exactly one of repo or path")
    fmt = dataset.get("format")
    if fmt not in DATASET_FORMATS:
        raise SpecError(
            "invalid_dataset",
            f"dataset.format '{fmt}' is not one of {', '.join(sorted(DATASET_FORMATS))}",
        )
    if method == "dpo" and fmt not in ("dpo", "jsonl"):
        raise SpecError("invalid_dataset", "method dpo needs dataset.format dpo (prompt, chosen, rejected)")
    if method != "dpo" and fmt == "dpo":
        raise SpecError("invalid_dataset", f"dataset.format dpo is only for method dpo, not {method}")
    out: Dict[str, Optional[str]] = {"path": None, "repo": None, "revision": dataset.get("revision"), "format": fmt}
    if path:
        if not isinstance(path, str):
            raise SpecError("invalid_dataset", "dataset.path must be a string")
        file = _expand(path)
        if file.suffix.lower() not in DATASET_SUFFIXES:
            raise SpecError("invalid_dataset", f"dataset file must be .jsonl, .json or .parquet: {file}")
        if not file.is_file():
            raise SpecError("invalid_dataset", f"dataset file not found: {file}")
        out["path"] = str(file.resolve())
    else:
        if not isinstance(repo, str) or _looks_like_path(repo) or not _REPO_ID.match(repo):
            raise SpecError("invalid_dataset", f"dataset.repo '{repo}' is not a Hugging Face dataset id")
        out["repo"] = repo
    return out


def _params(raw: Any) -> Dict[str, Any]:
    if raw is None:
        return {}
    if not isinstance(raw, dict):
        raise SpecError("invalid_spec", "params must be an object")
    unknown = sorted(set(raw) - set(_PARAM_TYPES))
    if unknown:
        raise SpecError(
            "invalid_spec",
            f"unknown params: {', '.join(unknown)} (known: {', '.join(KNOWN_PARAMS)})",
        )
    out: Dict[str, Any] = {}
    for key, value in raw.items():
        want = _PARAM_TYPES[key]
        if want is float and isinstance(value, int) and not isinstance(value, bool):
            value = float(value)
        if want is list and isinstance(value, str):
            if value != "all-linear":
                raise SpecError("invalid_spec", f"params.{key} must be a list of module names or \"all-linear\"")
        elif (want in (int, float) and isinstance(value, bool)) or not isinstance(value, want):
            raise SpecError("invalid_spec", f"params.{key} must be {want.__name__}")
        elif want is list and not all(isinstance(m, str) for m in value):
            raise SpecError("invalid_spec", f"params.{key} must be a list of module names")
        if want in (int, float) and value < 0:
            raise SpecError("invalid_spec", f"params.{key} must not be negative")
        out[key] = value
    for key in ("rank", "alpha", "lr", "batch_size", "grad_accum", "max_seq_len"):
        if key in out and out[key] <= 0:
            raise SpecError("invalid_spec", f"params.{key} must be greater than 0")
    if out.get("max_seq_len", 0) > MAX_SEQ_LEN:
        raise SpecError("invalid_spec", f"params.max_seq_len is above this engine's limit {MAX_SEQ_LEN}")
    if "dropout" in out and out["dropout"] >= 1:
        raise SpecError("invalid_spec", "params.dropout must be below 1")
    return out


def _outputs(raw: Any, method: str) -> Dict[str, Any]:
    raw = raw or {}
    if not isinstance(raw, dict):
        raise SpecError("invalid_spec", "outputs must be an object")
    if raw.get("mlx"):
        raise SpecError(
            "invalid_spec",
            "outputs.mlx is not available: the MLX backend of allternit-tune is waiting on a product decision",
        )
    gguf = raw.get("gguf") or []
    if not isinstance(gguf, list) or not all(isinstance(q, str) for q in gguf):
        raise SpecError("invalid_spec", "outputs.gguf must be a list of quant names")
    gguf = [q.lower() for q in gguf]
    bad = [q for q in gguf if q not in GGUF_DIRECT | GGUF_QUANTIZE]
    if bad:
        raise SpecError("invalid_spec", f"unknown GGUF quant: {', '.join(bad)}")
    if method == "full":
        # A full fine-tune has no adapter; the trained model is the merged output.
        return {"adapter": False, "merged": True, "gguf": gguf}
    # The adapter is the training result for LoRA methods, so it is always kept.
    return {"adapter": True, "merged": bool(raw.get("merged")), "gguf": gguf}


def build_plan(spec: Any, unavailable_methods: Optional[Dict[str, str]] = None) -> TrainPlan:
    """Check a job spec v1 and map it to a TrainPlan.

    `unavailable_methods` maps a method to the reason this computer cannot run
    it (from the hardware probe). Raises SpecError on the first refusal.
    """
    if not isinstance(spec, dict):
        raise SpecError("invalid_spec", "job spec must be a JSON object")
    unknown = sorted(set(spec) - TOP_LEVEL_KEYS)
    if unknown:
        raise SpecError("invalid_spec", f"unknown fields: {', '.join(unknown)}")
    for key in ("spec_version", "engine", "base", "dataset", "method", "resources"):
        if key not in spec:
            raise SpecError("invalid_spec", f"missing field: {key}")
    if spec["spec_version"] != SPEC_VERSION:
        raise SpecError(
            "unsupported_spec_version",
            f"spec_version {spec['spec_version']} is not supported (expected {SPEC_VERSION})",
        )
    if spec["engine"] != ENGINE_ID:
        raise SpecError("unknown_engine", f"this engine is {ENGINE_ID}, not '{spec['engine']}'")
    method = spec["method"]
    if method not in METHODS:
        raise SpecError("unknown_method", f"engine '{ENGINE_ID}' does not support method '{method}'")
    resources = spec["resources"]
    if not isinstance(resources, dict) or resources.get("place") not in PLACES:
        raise SpecError("invalid_spec", "resources.place must be local, paired, compute or rented")
    if resources["place"] == "rented":
        raise SpecError("rented_not_available", "rented GPUs are not available yet")
    if spec.get("publish") is not None:
        raise SpecError("publish_not_available", "publishing from a job is not available yet")
    if unavailable_methods and method in unavailable_methods:
        raise SpecError("unknown_method", f"{method} cannot run on this computer: {unavailable_methods[method]}")

    params = _params(spec.get("params"))
    dataset = _check_dataset(spec["dataset"], method)
    base = resolve_base(spec["base"])
    outputs = _outputs(spec.get("outputs"), method)

    max_hours = resources.get("max_hours")
    if max_hours is not None and (not isinstance(max_hours, (int, float)) or max_hours <= 0):
        raise SpecError("invalid_spec", "resources.max_hours must be a positive number")

    lora = None
    if method != "full":
        rank = params.get("rank", 16)
        lora = LoraSettings(
            r=rank,
            lora_alpha=params.get("alpha", rank * 2),
            lora_dropout=params.get("dropout", 0.0),
            target_modules=params.get("target_modules", "all-linear"),
        )

    max_seq_len = params.get("max_seq_len", 2048)
    training_args: Dict[str, Any] = {
        "learning_rate": params.get("lr", _DEFAULT_LR[method]),
        "num_train_epochs": params.get("epochs", 1.0),
        "max_steps": params.get("max_steps", -1),
        "per_device_train_batch_size": params.get("batch_size", 2),
        "gradient_accumulation_steps": params.get("grad_accum", 4),
        "warmup_steps": params.get("warmup_steps", 0),
        "weight_decay": params.get("weight_decay", 0.0),
        "lr_scheduler_type": params.get("lr_scheduler", "linear"),
        "seed": params.get("seed", 42),
        "gradient_checkpointing": params.get("gradient_checkpointing", False),
        "max_length": max_seq_len,
        "logging_steps": 1,
    }
    if method == "dpo":
        training_args["beta"] = params.get("beta", 0.1)
    elif "beta" in params:
        raise SpecError("invalid_spec", "params.beta is only for method dpo")

    notes: List[str] = []
    ev = spec.get("eval") or {}
    if isinstance(ev, dict) and ev.get("sets"):
        notes.append("eval sets are not run by allternit-tune 0.1; the job trains and exports only")

    return TrainPlan(
        method=method,
        base_path=str(base),
        dataset_path=dataset["path"],
        dataset_repo=dataset["repo"],
        dataset_revision=dataset["revision"],
        dataset_format=dataset["format"] or "jsonl",
        lora=lora,
        load_in_4bit=method == "qlora",
        training_args=training_args,
        max_seq_len=max_seq_len,
        beta=training_args.get("beta"),
        want_adapter=outputs["adapter"],
        want_merged=outputs["merged"],
        gguf=outputs["gguf"],
        max_hours=float(max_hours) if max_hours is not None else None,
        notes=notes,
    )
