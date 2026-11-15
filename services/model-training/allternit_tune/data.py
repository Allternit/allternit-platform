# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Allternit LLC
"""Load a dataset and shape its rows into what TRL expects.

Formats (dataset.format):
- chat: a `messages` column of {role, content}; ShareGPT `conversations`
  ({from, value}) is converted. Needs a base model with a chat template.
- alpaca: instruction / input / output -> prompt / completion.
- text: a `text` column.
- completion: prompt / completion columns.
- dpo: prompt / chosen / rejected columns.
- jsonl: detect one of the above from the columns.
"""

from __future__ import annotations

from typing import Any, Dict, List

ALPACA_PROMPT = "### Instruction:\n{instruction}\n\n{input_block}### Response:\n"

_SHAREGPT_ROLES = {"human": "user", "user": "user", "gpt": "assistant", "assistant": "assistant", "system": "system"}


class DataError(Exception):
    pass


def detect_format(columns: List[str]) -> str:
    cols = set(columns)
    if {"prompt", "chosen", "rejected"} <= cols:
        return "dpo"
    if "messages" in cols or "conversations" in cols:
        return "chat"
    if {"instruction", "output"} <= cols:
        return "alpaca"
    if {"prompt", "completion"} <= cols:
        return "completion"
    if "text" in cols:
        return "text"
    raise DataError(
        f"cannot tell the dataset format from its columns ({', '.join(sorted(cols))}); "
        "set dataset.format to chat, alpaca, text, completion or dpo"
    )


def _alpaca_row(row: Dict[str, Any]) -> Dict[str, str]:
    extra = (row.get("input") or "").strip()
    prompt = ALPACA_PROMPT.format(
        instruction=row.get("instruction") or "",
        input_block=f"### Input:\n{extra}\n\n" if extra else "",
    )
    return {"prompt": prompt, "completion": str(row.get("output") or "")}


def _sharegpt_row(row: Dict[str, Any]) -> Dict[str, Any]:
    messages = []
    for turn in row["conversations"]:
        role = _SHAREGPT_ROLES.get(str(turn.get("from", "")).lower())
        if role is None:
            raise DataError(f"unknown ShareGPT role: {turn.get('from')}")
        messages.append({"role": role, "content": turn.get("value", "")})
    return {"messages": messages}


_REQUIRED = {
    "chat": None,  # messages or conversations, checked below
    "alpaca": ("instruction", "output"),
    "text": ("text",),
    "completion": ("prompt", "completion"),
    "dpo": ("prompt", "chosen", "rejected"),
}


def shape(dataset: Any, fmt: str) -> Any:
    """Return (dataset with TRL columns, resolved format)."""
    columns = list(dataset.column_names)
    if fmt == "jsonl":
        fmt = detect_format(columns)
    if fmt == "chat":
        if "messages" in columns:
            keep = ["messages"]
        elif "conversations" in columns:
            dataset = dataset.map(_sharegpt_row, remove_columns=columns)
            return dataset, fmt
        else:
            raise DataError("format chat needs a messages (or conversations) column")
        return dataset.remove_columns([c for c in columns if c not in keep]), fmt
    missing = [c for c in _REQUIRED[fmt] if c not in columns]
    if missing:
        raise DataError(f"format {fmt} needs column(s): {', '.join(missing)}")
    if fmt == "alpaca":
        return dataset.map(_alpaca_row, remove_columns=columns), fmt
    keep = list(_REQUIRED[fmt])
    return dataset.remove_columns([c for c in columns if c not in keep]), fmt


def data_cache_dir(plan: Any) -> str:
    from pathlib import Path

    return str(Path(plan.job_dir) / ".data-cache")


def load(plan: Any) -> Any:
    """Load the plan's dataset (local file or Hub repo) as one train split."""
    from datasets import load_dataset

    if plan.dataset_path:
        path = plan.dataset_path
        builder = "parquet" if path.lower().endswith(".parquet") else "json"
        # Keep the Arrow copy of a private local file inside the job folder,
        # not in the shared Hugging Face cache; the backend removes it after.
        dataset = load_dataset(builder, data_files=path, split="train", cache_dir=data_cache_dir(plan))
    else:
        dataset = load_dataset(plan.dataset_repo, revision=plan.dataset_revision, split="train")
    if len(dataset) == 0:
        raise DataError("the dataset has no rows")
    return dataset
