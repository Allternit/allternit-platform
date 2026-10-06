# Core review findings — applied by orchestrator

- Probed system git in scratch bare repo: `show-ref --verify --hash refs/heads/main` returns 128 on unborn ref, not 1. Changed head query to `rev-parse --verify --quiet` (1 on missing, OID on present). Include regression test initialize.
- Published Agent Memory Repo format does not require `id`. Parser now derives deterministic entry identity when omitted, retaining required source/date for our product. Add test accepting external standard bullet without id and updating it via derived id. Do not revert these fixes while adding remaining functions/tests.

- New bare repos/index scratch dirs use Unix 0700; regression test added.
- Standard named entrypoint `# Memory: Joe` accepted. Optional-id import/delete regression test added.

Orchestrator review: inspected actual core footprint and tests; formatting/parser check and whitespace check pass. Core implementation may be used by the next implementation pass, but Rust compilation and behavioral verification remain pending; full Phase 1 is not accepted.
