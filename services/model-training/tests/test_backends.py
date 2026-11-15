# SPDX-License-Identifier: Apache-2.0
# Copyright 2026 Allternit LLC
"""Backend selection and refusals against the studio-spec fit table, torch
device mapping with mocked torch device probes, and the MLX backend's export
with a mocked mlx-lm. No torch, mlx or real training needed."""
from __future__ import annotations

import copy
import io
import json
import sys
import types
from pathlib import Path

import pytest

from allternit_tune import hardware
from allternit_tune.backends import select_backend
from allternit_tune.backends.mlx_backend import MlxBackend
from allternit_tune.events import Emitter, decode
from allternit_tune.hardware import Host, HardwareError
from allternit_tune.spec import SpecError, build_plan

FIT_TABLE = json.loads(
    (Path(__file__).resolve().parents[2] / "studio-spec" / "fixtures" / "fit-table.json").read_text()
)
TUNE = FIT_TABLE["engines"]["allternit-tune"]
CELLS = [(hw, m) for hw in FIT_TABLE["hardware"] for m in hardware.METHODS]


def for_method(spec: dict, method: str) -> dict:
    spec = copy.deepcopy(spec)
    spec["method"] = method
    spec["outputs"]["gguf"] = []
    if method == "dpo":
        spec["dataset"]["format"] = "dpo"
    return spec


# -- the fit table -------------------------------------------------------------

def test_hardware_ids_match_the_contract():
    assert list(hardware.HARDWARE) == FIT_TABLE["hardware"]


@pytest.mark.parametrize("hw, method", CELLS)
def test_job_on_each_cell_runs_or_is_refused_like_the_fit_table(spec, hw, method):
    cell = TUNE[hw]
    want = cell["methods"][method]
    job = for_method(spec, method)
    if want == "ok":
        plan = build_plan(job, Host(hw))
        assert plan.backend == cell["backend"][method]
        assert plan.hardware == hw
        assert select_backend(plan.backend).id == ("mlx" if plan.backend == "mlx" else "torch")
    else:
        with pytest.raises(SpecError) as info:
            build_plan(job, Host(hw))
        assert info.value.code == "method_not_supported"


@pytest.mark.parametrize("hw", FIT_TABLE["hardware"])
def test_mlx_output_runs_or_is_refused_like_the_fit_table(spec, hw):
    job = for_method(spec, "lora")
    job["outputs"]["mlx"] = True
    if TUNE[hw]["outputs"]["mlx"] == "ok":
        assert build_plan(job, Host(hw)).want_mlx
    else:
        with pytest.raises(SpecError) as info:
            build_plan(job, Host(hw))
        assert info.value.code == "output_not_supported"


@pytest.mark.parametrize("hw", FIT_TABLE["hardware"])
def test_engine_card_fit_matches_the_table(hw):
    cell, card = TUNE[hw], Host(hw).card()
    assert card["ok"] and card["hardware"] == hw
    assert set(card["methods_unavailable"]) == {m for m, v in cell["methods"].items() if v != "ok"}
    assert set(card["outputs_unavailable"]) == {o for o, v in cell["outputs"].items() if v != "ok"}
    assert (card["warning"] is None) == (cell["warning"] is None)
    assert card["backend"] == cell["backend"]["lora"]


@pytest.mark.parametrize(
    "hw, method, choice, backend",
    [
        ("apple-silicon", "lora", "torch", "torch-mps"),
        ("apple-silicon", "qlora", "torch", "torch-mps"),
        ("apple-silicon", "qlora", "mlx", "mlx"),
        ("apple-silicon", "full", "mlx", None),
        ("apple-silicon", "dpo", "mlx", None),
        ("cuda-linux", "lora", "mlx", None),
        ("xpu-windows", "lora", "torch", "torch-xpu"),
        ("rocm-linux", "qlora", "auto", "torch-rocm"),
    ],
)
def test_resources_backend_is_honoured(spec, hw, method, choice, backend):
    job = for_method(spec, method)
    job["resources"]["backend"] = choice
    if backend:
        assert build_plan(job, Host(hw)).backend == backend
    else:
        with pytest.raises(SpecError) as info:
            build_plan(job, Host(hw))
        assert info.value.code == "backend_not_supported"


# -- hardware id and torch devices (mocked torch device probes) ---------------

def fake_torch(*, cuda=False, hip=None, xpu=None, mps=False, version="2.15.0"):
    torch = types.ModuleType("torch")
    torch.__version__ = version
    torch.version = types.SimpleNamespace(hip=hip, cuda=None if hip else "13.4")
    torch.cuda = types.SimpleNamespace(is_available=lambda: cuda)
    torch.backends = types.SimpleNamespace(mps=types.SimpleNamespace(is_available=lambda: mps))
    if xpu is not None:
        torch.xpu = types.SimpleNamespace(is_available=lambda: xpu)
    return torch


@pytest.fixture
def machine(monkeypatch):
    def set_(system: str, arch: str = "x86_64", torch=None, bnb=None):
        monkeypatch.setattr(hardware.platform, "system", lambda: system)
        monkeypatch.setattr(hardware.platform, "machine", lambda: arch)
        monkeypatch.setitem(sys.modules, "torch", torch)
        if bnb is None:
            monkeypatch.setitem(sys.modules, "bitsandbytes", None)
        else:
            monkeypatch.setitem(sys.modules, "bitsandbytes", types.SimpleNamespace(__version__=bnb))
    monkeypatch.delenv("ALLTERNIT_STUDIO_HARDWARE", raising=False)
    return set_


def test_host_hardware_env_wins_and_bad_values_are_refused(monkeypatch, machine):
    machine("Linux", torch=fake_torch(cuda=True))
    monkeypatch.setenv("ALLTERNIT_STUDIO_HARDWARE", "xpu-linux")
    assert hardware.hardware_id() == "xpu-linux"
    monkeypatch.setenv("ALLTERNIT_STUDIO_HARDWARE", "Unsupported")
    with pytest.raises(HardwareError):
        hardware.hardware_id()


@pytest.mark.parametrize(
    "system, torch, want",
    [
        ("Linux", fake_torch(cuda=True), "cuda-linux"),
        ("Windows", fake_torch(cuda=True), "cuda-windows"),
        ("Linux", fake_torch(cuda=True, hip="7.2"), "rocm-linux"),
        ("Windows", fake_torch(cuda=True, hip="7.2"), "cpu"),
        ("Linux", fake_torch(xpu=True), "xpu-linux"),
        ("Windows", fake_torch(xpu=True), "xpu-windows"),
        ("Linux", fake_torch(), "cpu"),
    ],
)
def test_guess_without_a_host(machine, system, torch, want):
    machine(system, torch=torch)
    assert hardware.hardware_id() == want


@pytest.mark.parametrize(
    "hw, torch, ok",
    [
        ("rocm-linux", fake_torch(cuda=True, hip="7.2"), True),
        ("cuda-linux", fake_torch(cuda=True, hip="7.2"), False),  # a ROCm build on an NVIDIA id
        ("rocm-linux", fake_torch(cuda=True), False),  # a CUDA build on an AMD id
        ("xpu-linux", fake_torch(xpu=True), True),
        ("xpu-windows", fake_torch(), False),  # torch without XPU support
        ("cuda-windows", fake_torch(cuda=True), True),
        ("cpu", fake_torch(), True),
    ],
)
def test_torch_must_see_the_device_the_host_named(machine, spec, hw, torch, ok):
    machine("Windows" if hw.endswith("windows") else "Linux", torch=torch)
    job = for_method(spec, "lora")
    host = Host(hw, check_env=True)
    if ok:
        assert build_plan(job, host).backend == hardware.TORCH_BACKEND[hw]
    else:
        with pytest.raises(SpecError) as info:
            build_plan(job, host)
        assert info.value.code == "method_not_supported"
        assert hardware.REQUIREMENTS[hw] in info.value.message
        assert hardware.REQUIREMENTS[hw] in host.card()["methods_unavailable"]["lora"]


@pytest.mark.parametrize(
    "hw, bnb, ok",
    [
        ("cuda-linux", "0.45.0", True),
        ("xpu-linux", "0.49.1", False),
        ("xpu-linux", "0.50.2", True),
        ("rocm-linux", None, False),
    ],
)
def test_qlora_needs_bitsandbytes_that_runs_here(machine, spec, hw, bnb, ok):
    torch = fake_torch(cuda=hw != "xpu-linux", hip="7.2" if hw == "rocm-linux" else None, xpu=hw == "xpu-linux")
    machine("Linux", torch=torch, bnb=bnb)
    job = for_method(spec, "qlora")
    if ok:
        assert build_plan(job, Host(hw, check_env=True)).load_in_4bit
    else:
        with pytest.raises(SpecError) as info:
            build_plan(job, Host(hw, check_env=True))
        assert info.value.code == "method_not_supported" and "bitsandbytes" in info.value.message


def test_intel_mac_is_refused_with_a_reason(machine):
    machine("Darwin", "x86_64", torch=None)
    assert hardware.hardware_id() == "cpu"
    card = Host("cpu", check_env=True).card()
    assert not card["ok"] and card["backend"] is None
    assert "Intel Mac" in card["reason"]


def test_mac_without_mlx_lm_refuses_mlx_jobs(machine, monkeypatch, spec):
    machine("Darwin", "arm64", torch=fake_torch(mps=True), bnb="0.50.2")
    monkeypatch.setitem(sys.modules, "mlx_lm", None)
    host = Host("apple-silicon", check_env=True)
    with pytest.raises(SpecError) as info:
        build_plan(for_method(spec, "lora"), host)
    assert info.value.code == "backend_not_supported" and "mlx-lm" in info.value.message
    assert build_plan(for_method(spec, "full"), host).backend == "torch-mps"
    assert "mlx-lm" in host.card()["outputs_unavailable"]["mlx"]


# -- MLX backend with a mocked mlx-lm -----------------------------------------

class FakeLinear:
    def fuse(self, dequantize=False):
        return ("fused", dequantize)


class FakeModel:
    def __init__(self, path, adapter=None):
        self.path, self.adapter = path, adapter
        self.layers = [types.SimpleNamespace(named_modules=lambda: [("self_attn.q_proj", FakeLinear()),
                                                                     ("mlp.up_proj", FakeLinear())])]
        self.updated = None

    def named_modules(self):
        return [("layers.0.self_attn.q_proj", FakeLinear())]

    def update_modules(self, tree):
        self.updated = tree


@pytest.fixture
def mlx_lm(monkeypatch):
    """Stand-ins for the mlx / mlx-lm functions the backend calls."""
    calls = {"convert": [], "train": None, "save": []}

    def convert(hf_path, mlx_path="mlx_model", quantize=False, q_bits=None, q_group_size=None, **_):
        out = Path(mlx_path)
        out.mkdir(parents=True)
        config = {"model_type": "llama"}
        if quantize:
            config["quantization"] = {"bits": q_bits, "group_size": q_group_size}
        (out / "config.json").write_text(json.dumps(config))
        (out / "model.safetensors").write_bytes(b"w" * (4 if quantize else 16))
        calls["convert"].append({"src": hf_path, "dst": str(out), "quantize": quantize, "bits": q_bits})

    def load(path, adapter_path=None, return_config=False, **_):
        model = FakeModel(path, adapter_path)
        if return_config:
            return model, "tok", json.loads((Path(path) / "config.json").read_text())
        return model, "tok"

    def load_dataset(args, tokenizer):
        rows = [json.loads(line) for line in (Path(args.data) / "train.jsonl").read_text().splitlines()]
        return rows, [], []

    def train_model(args, model, train_set, valid_set, callback):
        calls["train"] = {"args": vars(args), "rows": train_set}
        Path(args.adapter_path).mkdir(parents=True, exist_ok=True)
        (Path(args.adapter_path) / "adapters.safetensors").write_bytes(b"a" * 8)
        callback.on_train_loss_report({"iteration": args.iters, "train_loss": 1.25})

    def save(dst, src, model, tokenizer, config, donate_model=True):
        dst = Path(dst)
        dst.mkdir(parents=True)
        (dst / "config.json").write_text(json.dumps(config))
        (dst / "model.safetensors").write_bytes(b"m" * 32)
        calls["save"].append({"dst": str(dst), "fused": model.updated, "config": config})

    mods = {
        "mlx": types.ModuleType("mlx"),
        "mlx.utils": types.SimpleNamespace(tree_unflatten=lambda pairs: dict(pairs)),
        "mlx_lm": types.ModuleType("mlx_lm"),
        "mlx_lm.convert": types.SimpleNamespace(convert=convert),
        "mlx_lm.lora": types.SimpleNamespace(
            CONFIG_DEFAULTS={"optimizer_config": {"adam": {}, "adamw": {}}, "lora_parameters": {"rank": 8}},
            train_model=train_model),
        "mlx_lm.tuner": types.ModuleType("mlx_lm.tuner"),
        "mlx_lm.tuner.datasets": types.SimpleNamespace(load_dataset=load_dataset),
        "mlx_lm.tuner.trainer": types.SimpleNamespace(TrainingCallback=type("TrainingCallback", (), {})),
        "mlx_lm.utils": types.SimpleNamespace(dequantize_model=lambda m: m, load=load, save=save),
    }
    for name, mod in mods.items():
        monkeypatch.setitem(sys.modules, name, mod)
    return calls


def run_mlx(plan):
    stream = io.StringIO()
    MlxBackend().run(plan, Emitter(stream))
    return [decode(line) for line in stream.getvalue().splitlines()]


def test_mlx_qlora_writes_adapter_merged_and_4bit_mlx_under_the_output_dir(spec, tmp_path, mlx_lm):
    job = for_method(spec, "qlora")
    job["outputs"].update(merged=True, mlx=True)
    job["params"]["target_modules"] = ["q_proj"]
    plan = build_plan(job, Host("apple-silicon"))
    assert plan.backend == "mlx"
    plan.job_dir = str(tmp_path / "job")
    plan.output_dir = str(tmp_path / "out")
    Path(plan.job_dir).mkdir()

    events = run_mlx(plan)

    outputs = {e["output"]["kind"]: e["output"] for e in events if e["kind"] == "output"}
    assert set(outputs) == {"adapter", "merged", "mlx"}
    for out in outputs.values():
        assert Path(out["path"]).is_absolute() and out["path"].startswith(str((tmp_path / "out").resolve()))
        assert out["bytes"] > 0
    # QLoRA: the base is converted to 4-bit first; MLX output keeps it, merged is dequantized.
    assert mlx_lm["convert"] == [{"src": plan.base_path, "dst": str(Path(plan.job_dir) / ".mlx-base-4bit"),
                                  "quantize": True, "bits": 4}]
    mlx_config = json.loads((Path(outputs["mlx"]["path"]) / "config.json").read_text())
    merged_config = json.loads((Path(outputs["merged"]["path"]) / "config.json").read_text())
    assert mlx_config["quantization"]["bits"] == 4
    assert "quantization" not in merged_config
    assert all(s["fused"] for s in mlx_lm["save"])

    args = mlx_lm["train"]["args"]
    assert args["fine_tune_type"] == "lora" and args["num_layers"] == -1
    assert args["lora_parameters"] == {"rank": 16, "dropout": 0.0, "scale": 2.0, "keys": ["self_attn.q_proj"]}
    assert args["iters"] == 3 and args["batch_size"] == 2  # 5 rows, batch 2, 1 epoch
    assert mlx_lm["train"]["rows"][0] == {"prompt": "### Instruction:\nSay 0\n\n### Response:\n", "completion": "0"}

    states = [e["state"] for e in events if e["kind"] == "state"]
    assert states == ["running", "exporting"]
    assert [e["progress"] for e in events if e["kind"] == "progress"] == [1.0]
    assert not (Path(plan.job_dir) / ".mlx-base-4bit").exists()
    assert not (Path(plan.job_dir) / ".mlx-data").exists()


def test_mlx_lora_without_extra_outputs_writes_only_the_adapter(spec, tmp_path, mlx_lm):
    plan = build_plan(for_method(spec, "lora"), Host("apple-silicon"))
    plan.job_dir = plan.output_dir = str(tmp_path)
    events = run_mlx(plan)
    assert [e["output"]["kind"] for e in events if e["kind"] == "output"] == ["adapter"]
    assert mlx_lm["convert"] == []  # LoRA trains on the base as it is


def test_mlx_backend_refuses_methods_mlx_lm_cannot_train(spec, tmp_path, mlx_lm):
    plan = build_plan(for_method(spec, "full"), Host("apple-silicon"))
    plan.backend, plan.job_dir = "mlx", str(tmp_path)
    with pytest.raises(RuntimeError, match="PyTorch"):
        MlxBackend().run(plan, Emitter(io.StringIO()))
