# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Allternit LLC
"""Dataset shaping into the columns TRL trains on."""
from __future__ import annotations

import pytest

datasets = pytest.importorskip("datasets")

from allternit_tune.data import DataError, shape  # noqa: E402


def test_alpaca_becomes_prompt_completion():
    ds = datasets.Dataset.from_list([{"instruction": "Add 2+2", "input": "", "output": "4", "id": 1}])
    out, fmt = shape(ds, "jsonl")
    assert fmt == "alpaca"
    assert out.column_names == ["prompt", "completion"]
    assert out[0]["prompt"] == "### Instruction:\nAdd 2+2\n\n### Response:\n"
    assert out[0]["completion"] == "4"


def test_sharegpt_becomes_messages():
    ds = datasets.Dataset.from_list([{"conversations": [{"from": "human", "value": "hi"}, {"from": "gpt", "value": "yo"}]}])
    out, fmt = shape(ds, "chat")
    assert out[0]["messages"] == [{"role": "user", "content": "hi"}, {"role": "assistant", "content": "yo"}]


def test_dpo_detected_and_missing_columns_named():
    ds = datasets.Dataset.from_list([{"prompt": "p", "chosen": "a", "rejected": "b", "extra": 1}])
    out, fmt = shape(ds, "jsonl")
    assert fmt == "dpo" and out.column_names == ["prompt", "chosen", "rejected"]
    with pytest.raises(DataError, match="needs column"):
        shape(datasets.Dataset.from_list([{"prompt": "p"}]), "completion")
