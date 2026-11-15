<!-- SPDX-License-Identifier: Apache-2.0 -->
# allternit-tune

Allternit's own fine-tuning engine, the default engine of Freeweights Studio.
Apache-2.0 (see `LICENSE` and `NOTICE`). It speaks engine API v1
(`services/studio-spec/openapi/engine-api.yaml`) and trains with TRL + PEFT on
torch. Python package: `allternit_tune`.

| Method | What it does | CUDA (Linux) | Apple Silicon (MPS) |
|---|---|---|---|
| `lora` | PEFT LoRA adapters, TRL SFTTrainer | yes | yes (verified) |
| `qlora` | base loaded 4-bit with bitsandbytes, then LoRA | yes | yes with bitsandbytes 0.50+ (verified) |
| `dpo` | TRL DPOTrainer with LoRA adapters | yes | yes (verified) |
| `full` | every weight trains, no adapter | yes | yes (verified; memory bound) |

The MLX backend is waiting on a product decision (Decision 3 of the Studio
Phase 3 plan). The backend interface is `allternit_tune/backends/__init__.py`;
`MlxBackend` is registered as `pending_decision` and the engine card says so.

## Run the engine

```
python -m allternit_tune.server --port 0 --token <per-launch-token> [--data-dir DIR]
```

- Binds 127.0.0.1 only. Every request needs `Authorization: Bearer <token>`.
  `ALLTERNIT_TUNE_TOKEN` works instead of `--token`, which keeps the token out of `ps`.
- Prints one line `{"ready": true, "port": N, ...}` when it is listening.
- Job folders go to `--data-dir` (default `~/.allternit/studio/allternit-tune`):
  `jobs/<id>/adapter`, `merged`, `gguf`, plus `plan.json`, `events.jsonl`, `worker.log`.
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
  (`f16`, `q8_0`, `q4_k_m`, ...). `mlx: true` is refused until the MLX backend exists.
- `resources.max_hours` stops the job when reached.

The card (`GET /v1/engine`) carries `limits.device` and
`limits.unavailable_here` (method -> reason) for this computer. A method that
cannot run here is refused at submit with that reason.

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
