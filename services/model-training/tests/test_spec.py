# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Allternit LLC
"""Job spec v1 -> training plan mapping and refusals."""
from __future__ import annotations

import copy

import pytest

from allternit_tune.hardware import Host
from allternit_tune.spec import SpecError, TrainPlan, build_plan

GPU = Host("cuda-linux")


def refused(spec: dict, host: Host = GPU) -> SpecError:
    with pytest.raises(SpecError) as info:
        build_plan(spec, host)
    return info.value


def test_lora_spec_maps_to_training_args(spec, base_dir, dataset_file):
    plan = build_plan(spec, GPU)
    assert plan.method == "lora"
    assert plan.base_path == str(base_dir.resolve())
    assert plan.dataset_path == str(dataset_file.resolve())
    assert (plan.lora.r, plan.lora.lora_alpha, plan.lora.target_modules) == (16, 32, "all-linear")
    assert plan.training_args["learning_rate"] == 0.0002
    assert plan.training_args["num_train_epochs"] == 1.0
    assert plan.training_args["max_length"] == 2048
    assert plan.training_args["max_steps"] == -1
    assert "beta" not in plan.training_args
    assert plan.want_adapter and not plan.want_merged
    assert plan.gguf == ["q4_k_m"]
    assert plan.max_hours == 1.0 and not plan.load_in_4bit


def test_defaults_per_method(spec):
    spec["params"] = {}
    spec["method"] = "full"
    full = build_plan(spec, GPU)
    assert full.lora is None
    assert full.training_args["learning_rate"] == 2e-5
    # A full fine-tune has no adapter; its model is the merged output.
    assert (full.want_adapter, full.want_merged) == (False, True)

    spec["method"] = "qlora"
    qlora = build_plan(spec, GPU)
    assert qlora.load_in_4bit and qlora.lora.r == 16 and qlora.lora.lora_alpha == 32

    dpo = copy.deepcopy(spec)
    dpo["method"] = "dpo"
    dpo["dataset"]["format"] = "dpo"
    dpo["params"] = {"beta": 0.2, "max_steps": 20}
    plan = build_plan(dpo, GPU)
    assert plan.training_args["beta"] == 0.2 and plan.training_args["max_steps"] == 20


def test_plan_round_trips_through_json(spec):
    plan = build_plan(spec, GPU)
    assert TrainPlan.from_dict(plan.to_dict()) == plan


def test_refuses_repo_id_base(spec):
    spec["base"]["repo"] = "HuggingFaceTB/SmolLM2-135M"
    err = refused(spec)
    assert err.code == "invalid_spec"
    assert "never downloads weights" in err.message


def test_refuses_missing_base_folder(spec, tmp_path):
    spec["base"]["repo"] = str(tmp_path / "nope")
    assert "not found" in refused(spec).message


@pytest.mark.parametrize(
    "mutate, code",
    [
        (lambda s: s.update(spec_version=2), "unsupported_spec_version"),
        (lambda s: s.update(engine="axolotl"), "unknown_engine"),
        (lambda s: s.update(method="rlhf"), "unknown_method"),
        (lambda s: s["resources"].update(place="rented"), "rented_not_available"),
        (lambda s: s.update(publish={"repo": "x/y"}), "publish_not_available"),
        (lambda s: s["dataset"].update(repo="org/data"), "invalid_dataset"),
        (lambda s: s["dataset"].update(format="parquetish"), "invalid_dataset"),
        (lambda s: s.update(surprise=1), "invalid_spec"),
        (lambda s: s["params"].update(rnak=8), "invalid_spec"),
        (lambda s: s["params"].update(rank="16"), "invalid_spec"),
        (lambda s: s["params"].update(max_seq_len=100000), "invalid_spec"),
        (lambda s: s["params"].update(beta=0.1), "invalid_spec"),
        (lambda s: s["outputs"].update(mlx=True), "output_not_supported"),
        (lambda s: s["resources"].update(backend="mlx"), "backend_not_supported"),
        (lambda s: s["resources"].update(backend="jax"), "invalid_spec"),
        (lambda s: s["outputs"].update(gguf=["q9_x"]), "invalid_spec"),
    ],
)
def test_refusal_codes(spec, mutate, code):
    mutate(spec)
    assert refused(spec).code == code


def test_dataset_repo_and_missing_file(spec, tmp_path):
    spec["dataset"] = {"repo": "tatsu-lab/alpaca", "format": "alpaca"}
    assert build_plan(spec, GPU).dataset_repo == "tatsu-lab/alpaca"
    spec["dataset"] = {"path": str(tmp_path / "missing.jsonl"), "format": "alpaca"}
    assert refused(spec).code == "invalid_dataset"


def test_cpu_plan_carries_the_slow_warning(spec):
    plan = build_plan(spec, Host("cpu"))
    assert plan.backend == "torch-cpu"
    assert any("slow" in note for note in plan.notes)
