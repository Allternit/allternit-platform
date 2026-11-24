# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Allternit LLC
from __future__ import annotations

import json
from pathlib import Path

import pytest


@pytest.fixture
def base_dir(tmp_path: Path) -> Path:
    """A local base-model folder as the host would hand it over."""
    folder = tmp_path / "base-model"
    folder.mkdir()
    (folder / "config.json").write_text(json.dumps({"model_type": "llama"}))
    return folder


@pytest.fixture
def dataset_file(tmp_path: Path) -> Path:
    path = tmp_path / "train.jsonl"
    rows = [{"instruction": f"Say {i}", "input": "", "output": str(i)} for i in range(5)]
    path.write_text("\n".join(json.dumps(r) for r in rows) + "\n")
    return path


@pytest.fixture
def spec(base_dir: Path, dataset_file: Path) -> dict:
    """The studio-spec lora-valid fixture with local paths filled in."""
    return {
        "spec_version": 1,
        "engine": "allternit-tune",
        "engine_version": "0.1.0",
        "base": {"repo": str(base_dir), "revision": "main", "manifest_sha256": "0" * 64},
        "dataset": {"path": str(dataset_file), "format": "alpaca"},
        "method": "lora",
        "params": {"rank": 16, "alpha": 32, "lr": 0.0002, "epochs": 1, "max_seq_len": 2048},
        "resources": {"place": "local", "max_hours": 1},
        "outputs": {"adapter": True, "merged": False, "gguf": ["q4_k_m"], "mlx": False},
        "eval": {"sets": [], "baseline": "base"},
    }
