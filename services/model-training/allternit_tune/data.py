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


def read_rows(path: str) -> List[Dict[str, Any]]:
    """Rows of a local .jsonl or .json file, without the datasets library."""
    import json

    with open(path, encoding="utf-8") as fh:
        if path.lower().endswith(".jsonl"):
            rows = [json.loads(line) for line in fh if line.strip()]
        else:
            rows = json.load(fh)
    if not isinstance(rows, list) or not all(isinstance(r, dict) for r in rows):
        raise DataError("the dataset file must hold one JSON object per row")
    if not rows:
        raise DataError("the dataset has no rows")
    return rows


def shape_rows(rows: List[Dict[str, Any]], fmt: str) -> Any:
    """`shape` for a plain list of rows: (rows with TRL columns, format).
    These are also the columns mlx-lm reads (text, prompt/completion, messages)."""
    columns = sorted({key for row in rows for key in row})
    if fmt == "jsonl":
        fmt = detect_format(columns)
    if fmt == "chat":
        if "messages" in columns:
            return [{"messages": r["messages"]} for r in rows], fmt
        if "conversations" in columns:
            return [_sharegpt_row(r) for r in rows], fmt
        raise DataError("format chat needs a messages (or conversations) column")
    missing = [c for c in _REQUIRED[fmt] if c not in columns]
    if missing:
        raise DataError(f"format {fmt} needs column(s): {', '.join(missing)}")
    if fmt == "alpaca":
        return [_alpaca_row(r) for r in rows], fmt
    return [{c: r.get(c) for c in _REQUIRED[fmt]} for r in rows], fmt
