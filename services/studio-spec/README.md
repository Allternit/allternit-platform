# allternit-studio-spec

The contract for Freeweights Studio training jobs. Apache-2.0. It holds:

- `src/spec.rs`: job spec v1 types and `JobSpec::validate`. Refusals carry stable codes: `rented_not_available`, `publish_not_available`, `unknown_engine`, `unknown_method`, `unsupported_spec_version`, `invalid_dataset`, `invalid_spec`.
- `src/job.rs`: `JobState` (queued, preparing, running, exporting, succeeded, failed, cancelled, forward-only), `Job`, `JobEvent`, `JobOutput`.
- `engines.json` + `src/engines.rs`: the engine catalog (`EngineInfo`). The MLX backend of allternit-tune is `pending_decision` until Eoj confirms the widened F14-prime.
- `openapi/studio-api.yaml`: host API under `/api/v1/studio`. `openapi/engine-api.yaml`: engine API v1 over loopback with a per-launch bearer token.
- `fixtures/`: one valid LoRA spec and one invalid spec per refusal.

## Fake engine for other lanes

A small engine API v1 server that walks a job through queued, preparing, running, exporting, succeeded and records events (seq starts at 1, +1 each).

```
cargo run -p allternit-studio-spec --example fake_engine -- 8765 mytoken
curl -H 'Authorization: Bearer mytoken' http://127.0.0.1:8765/v1/engine
curl -H 'Authorization: Bearer mytoken' -d @services/studio-spec/fixtures/lora-valid.json http://127.0.0.1:8765/v1/jobs
curl -H 'Authorization: Bearer mytoken' 'http://127.0.0.1:8765/v1/jobs/job_0001/events?after=0'
```

In Rust tests use `allternit_studio_spec::fake_engine::FakeEngine::start(token, step_delay)`; it binds a free loopback port. A second job while one runs returns 409 `job_running`; a bad token returns 401.
