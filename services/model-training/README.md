<!-- SPDX-License-Identifier: Apache-2.0 -->
# allternit-tune

Allternit's own fine-tuning engine, the default engine of Freeweights Studio.
Apache-2.0 (see `LICENSE` and `NOTICE`). It speaks engine API v1
(`services/studio-spec/openapi/engine-api.yaml`) and trains with TRL + PEFT on
torch. Python package: `allternit_tune`.

Every hardware id in the contract (`services/studio-spec/fixtures/fit-table.json`):

| Hardware id | Backend | Methods | QLoRA through |
|---|---|---|---|
| `apple-silicon` | `mlx` for lora/qlora, `torch-mps` for full/dpo | all four | MLX 4-bit (or bitsandbytes on MPS with `resources.backend: torch`) |
| `cuda-linux`, `cuda-windows` | `torch-cuda` | all four | bitsandbytes |
| `rocm-linux` | `torch-rocm` (torch reports it as cuda with `torch.version.hip`) | all four | bitsandbytes 0.50+ |
| `xpu-linux`, `xpu-windows` | `torch-xpu` | all four | bitsandbytes 0.50+ |
| `cpu` | `torch-cpu` | lora only, slow (`fit.warning`) | refused |

| Method | torch | MLX (mlx-lm 0.32.2) |
|---|---|---|
| `lora` | PEFT LoRA, TRL SFTTrainer | `mlx_lm` LoRA on every layer |
| `qlora` | base loaded 4-bit with bitsandbytes, then LoRA | base converted to 4-bit MLX in the job folder, then LoRA |
| `dpo` | TRL DPOTrainer with LoRA adapters | refused: mlx-lm has no DPO |
| `full` | every weight trains, no adapter | refused: Studio runs full on torch-mps |

`resources.backend` is `auto` (the row above), `torch` or `mlx`; `mlx` off a Mac
or for full/dpo is 422 `backend_not_supported`. A method the hardware cannot
run is 422 `method_not_supported`, MLX output off a Mac 422
`output_not_supported`, each with the reason from the fit. The same codes
carry what the engine's environment is missing (torch cannot see the GPU the
host named, bitsandbytes missing or too old, mlx-lm missing). Intel Macs
(`cpu`) are refused: PyTorch's last Intel Mac build is 2.2 and transformers 5
needs 2.6. Not tested by Allternit yet: ROCm, XPU, Windows, CPU, MLX.

The MLX adapter is mlx-lm's own format (`adapters.safetensors` +
`adapter_config.json`), not a PEFT adapter. `merged` is the fused, dequantized
model; `mlx` is the fused model (4-bit after QLoRA). On a PyTorch run with
`outputs.mlx` (full or DPO on a Mac) the merged model is converted with
`mlx_lm.convert`. GGUF always goes through llama.cpp.

## Requirements per hardware

The host installs `requirements-<hardware id>.txt` into the engine's venv:

| File | torch | Index |
|---|---|---|
| `requirements-apple-silicon.txt` | 2.15.0 + mlx-lm 0.32.2 | PyPI |
| `requirements-cuda-linux.txt`, `-cuda-windows.txt` | 2.15.0+cu134 | download.pytorch.org/whl/cu134 |
| `requirements-rocm-linux.txt` | 2.14.1+rocm7.2 (newest ROCm build) | download.pytorch.org/whl/rocm7.2 |
| `requirements-xpu-linux.txt`, `-xpu-windows.txt` | 2.15.0+xpu | download.pytorch.org/whl/xpu |
| `requirements-cpu.txt` | 2.15.0+cpu (not macOS) | download.pytorch.org/whl/cpu |

`requirements.txt` stays the generic set (torch from PyPI).

## Run the engine

```
python -m allternit_tune.server --port 0 --token <per-launch-token> [--data-dir DIR]
```

- Binds 127.0.0.1 only. Every request needs `Authorization: Bearer <token>`.
  `ALLTERNIT_TUNE_TOKEN` works instead of `--token`, which keeps the token out of `ps`.
- Prints one line `{"ready": true, "port": N, ...}` when it is listening.
- Job folders go to `--data-dir` (default `~/.allternit/studio/allternit-tune`):
  `jobs/<id>/plan.json`, `events.jsonl`, `worker.log`. Outputs (`adapter`,
  `merged`, `gguf`, `mlx`) go to `ALLTERNIT_STUDIO_OUTPUT_DIR` when the host
  sets it, else into the job folder. Output paths in events are absolute.
- `ALLTERNIT_STUDIO_HARDWARE` is the hardware id the host detected. Without
  it (a run by hand) the engine guesses from the OS and torch.
- SIGTERM cancels the running job (its worker is stopped) and exits.

Routes: `GET /v1/engine`, `POST /v1/jobs`, `GET /v1/jobs/{id}`,
`GET /v1/jobs/{id}/events?after=N`, `POST /v1/jobs/{id}/cancel`. One job at a
time (409 `job_running`). Refusals are 422 with the studio-api codes.

Each job runs in its own worker process (`python -m allternit_tune.worker`), so
cancel stops training at once and a torch crash cannot take the server down.

## What the host sends

- `base.repo` is the **local folder** of the base model. The host fetches the
  model and checks its sha256; this engine never downloads weights and refuses
  a repo id with a plain `invalid_spec` message.
- `dataset` is a local `.jsonl`, `.json` or `.parquet` file (`path`) or a
  Hugging Face dataset id (`repo`). `format`: `chat`, `alpaca`, `text`,
  `completion`, `dpo`, or `jsonl` to detect from the columns.
- `params`: `rank`, `alpha`, `dropout`, `target_modules`, `lr`, `epochs`,
  `max_steps`, `batch_size`, `grad_accum`, `warmup_steps`, `weight_decay`,
  `max_seq_len` (max 8192), `seed`, `lr_scheduler`, `gradient_checkpointing`,
  `beta` (dpo only). Unknown params are refused, so a typo is never ignored.
- `outputs`: the adapter is always written for LoRA methods; `merged` writes a
  merged safetensors model in the base model's dtype; `gguf` lists quants
  (`f16`, `q8_0`, `q4_k_m`, ...). `mlx: true` writes MLX files (Apple Silicon only).
- `resources.max_hours` stops the job when reached.

The card (`GET /v1/engine`) carries `fit` for this computer (`ok`,
`hardware`, `backend`, `reason`, `warning`, `methods_unavailable`,
`outputs_unavailable`). A job is refused at submit with those reasons.

## GGUF

GGUF export runs llama.cpp's `convert_hf_to_gguf.py` (plus `llama-quantize`
for k-quants) when they are on the computer: on `PATH`, in
`ALLTERNIT_LLAMA_CPP_DIR` / `LLAMA_CPP_DIR`, or in `~/llama.cpp`. The converter
needs llama.cpp's `gguf-py` installed in the engine's environment. When the
converter is missing the job logs "GGUF export needs llama.cpp" and still
succeeds with its other outputs. Homebrew's llama.cpp ships `llama-quantize`
but not the converter.

## Apple Silicon

Verified on an M1 Pro (32 GB), torch 2.15.0, transformers 5.23.0, peft 0.21.3,
trl 1.18.0, bitsandbytes 0.50.2: a 20-step run of each method on
SmolLM2-135M through the engine API. MPS trains in fp32 (MPS mixed precision is
unreliable); outputs are saved in the base model's dtype. `PYTORCH_ENABLE_MPS_FALLBACK=1`
is set in the worker so an op MPS lacks runs on CPU instead of failing.

## Develop

```
uv venv .scratch/venv && VIRTUAL_ENV=.scratch/venv uv pip install -r requirements.txt -r requirements-dev.txt
.scratch/venv/bin/python -m pytest
python3 tools/spdx_gate.py
```

`.scratch/` is git-ignored. The test suite runs the license gate too.
`tools/spdx_gate.py` fails when a file lacks an Apache-2.0-compatible SPDX
header or when code imports, or a requirements file lists, the AGPL trainer
library this package replaced (see `NOTICE`).
