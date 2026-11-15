# allternit-studio-spec

The contract for Freeweights Studio training jobs. Apache-2.0. It holds:

- `src/spec.rs`: job spec v1 types and `JobSpec::validate`. Refusals carry stable codes: `rented_not_available`, `publish_not_available`, `unknown_engine`, `unknown_method`, `unsupported_spec_version`, `invalid_base`, `invalid_dataset`, `invalid_spec`. `base` and `dataset` each take exactly one of `repo` or `path`; `resources.backend` is `auto` (default), `torch` or `mlx`.
- `src/hardware.rs`: the hardware ids `apple-silicon`, `cuda-linux`, `cuda-windows`, `rocm-linux`, `xpu-linux`, `xpu-windows`, `cpu`, and `classify(os, arch, gpus)`, which maps what the host found to one id. AMD GPUs on Windows and Intel Macs map to `cpu` with a reason. On an Intel Mac `HostHardware.torch_unavailable` is also set (the last Intel-Mac PyTorch is 2.2.2; the engines need 2.6+), and every torch-based engine's `fit` is `ok: false` with "Training needs PyTorch 2.6 or newer, which has no build for Intel Macs." The host does the probing.
- `src/fit.rs`: `fit(engine, host)` builds the `fit` object on an engine card (`ok`, `hardware`, `backend`, `reason`, `warning`, `methods_unavailable`, `outputs_unavailable`). `check_job(spec, host)` refuses a job with `hardware_not_supported` (the engine does not run here), `method_not_supported`, `backend_not_supported` or `output_not_supported`, each with the reason from `fit`. On a Mac, `auto` trains LoRA and QLoRA with MLX and full/DPO with PyTorch (MPS). MLX output is Apple Silicon only. On `cpu` only LoRA runs, and `fit.warning` says it is slow.
- `src/job.rs`: `JobState` (queued, preparing, running, exporting, succeeded, failed, cancelled, forward-only), `Job`, `JobEvent`, `JobOutput`.
- `engines.json` + `src/engines.rs`: the engine catalog (`EngineInfo`): the hardware each engine runs on and the pinned upstream version. allternit-tune runs on every id (PyTorch everywhere, MLX on a Mac). `notes` is shown on the card (torchtune is unmaintained upstream; Unsloth has no DPO).
- `openapi/studio-api.yaml`: host API under `/api/v1/studio`. `openapi/engine-api.yaml`: engine API v1 over loopback with a per-launch bearer token. The host launches an engine with `ALLTERNIT_STUDIO_OUTPUT_DIR` and `ALLTERNIT_STUDIO_HARDWARE`.
- `fixtures/`: valid LoRA specs (`lora-valid.json` from the hub, `lora-valid-local.json` with a local base and dataset) and one invalid spec per refusal.

Output paths (`JobOutput.path`) are always absolute paths on the computer that ran the job, inside the output dir the host gave the engine.

## The fit table

`fixtures/fit-table.json` says which engine runs which method on which hardware. The Rust tests (`tests/contract.rs`) and the allternit-tune Python tests both read it. Shape:

- `hardware`: every hardware id, in order.
- `engines.<engine>.<hardware>`: either a string, meaning the whole engine is refused there (`hardware_not_supported`, with this reason), or an object:
  - `methods`: `{method: "ok" | reason}`. A reason means `method_not_supported` with that message.
  - `backend`: `{method: backend}` for each method that runs, with `resources.backend` set to `auto`.
  - `outputs`: `{"mlx": "ok" | reason}`. A reason means `output_not_supported`.
  - `warning`: `null`, or the text shown before start.
- `classify`: `{os, arch, gpus, hardware, cpu_reason}` cases for `classify`. `cpu_reason: true` means the computer falls back to `cpu` and the host must say why. `torch_unavailable` (only on the Intel Mac case) is the exact `fit.reason` every torch-based engine gets there, with `ok: false`.

Change engines.json or `src/fit.rs` and the table together. Reasons are the exact words people see.

## Fake engine for other lanes

A small engine API v1 server that walks a job through queued, preparing, running, exporting, succeeded and records events (seq starts at 1, +1 each).

```
cargo run -p allternit-studio-spec --example fake_engine -- 8765 mytoken
curl -H 'Authorization: Bearer mytoken' http://127.0.0.1:8765/v1/engine
curl -H 'Authorization: Bearer mytoken' -d @services/studio-spec/fixtures/lora-valid.json http://127.0.0.1:8765/v1/jobs
curl -H 'Authorization: Bearer mytoken' 'http://127.0.0.1:8765/v1/jobs/job_0001/events?after=0'
```

The example takes a third argument, the hardware id to pretend to be (default `apple-silicon`, or `ALLTERNIT_STUDIO_HARDWARE`), and puts output paths under `ALLTERNIT_STUDIO_OUTPUT_DIR` when set. On `cpu`, a `qlora` job returns 422 `method_not_supported` with the reason from the fit table.

In Rust tests use `allternit_studio_spec::fake_engine::FakeEngine::start(token, step_delay)`, or `start_with(token, step_delay, FakeConfig { hardware, output_dir })`; it binds a free loopback port. A second job while one runs returns 409 `job_running`; a bad token returns 401. Without `output_dir`, outputs point under `<temp dir>/allternit-fake-engine`.
