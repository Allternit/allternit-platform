# ENVAPI notes — code environments backend (Contract B)

Branch `factory/cp-envapi`. Code only: nothing was built, typechecked or tested (per the map). The orchestrator's build should run `cargo test -p allternit-cloud-api code_environments` (pure validation tests, no DB) and the existing `migrations_pg_applies_cleanly` test.

## Built
- `cmd/allternit-cloud-api/migrations_pg/095_code_environments.sql`: additive. `code_environments` (jsonb `allowed_hosts`/`repos`, `archived_at`, partial unique index = one default per user) and `code_environment_secrets` (`value_ciphertext` + 12-byte GCM `value_nonce`, `hosts`, PK `(environment_id, name)`, cascade delete).
- `cmd/allternit-cloud-api/src/routes/code_environments.rs`: every Contract B route, registered in `routes/mod.rs` and merged into the public runtime router in `src/lib.rs` (next to `registered_phones`, same rate-limit layer).
  - Auth: Clerk session or `compute`-scoped `allternit_*` API token (`auth::resolve_user_scoped`), or a runtime device token acting as its owner (same trust as `runtime_pairing::approver_from_headers`, so the Desktop/runtime can call it too). Rows scoped by user id only (no org scoping: nothing comparable for per-user data in this crate). Wrong owner = 404.
  - `GET /` auto-creates "Default" (full network, is_default) when the user has no live (non-archived) env; race-safe via the one-default index + ON CONFLICT DO NOTHING. Sorted default first, then oldest.
  - `POST /` returns 201. Missing `allowed_hosts` on create = `DEFAULT_ALLOWED_HOSTS` (GitHub + npm/Yarn/Bun/PyPI/crates/Go/RubyGems/Maven/Debian/Ubuntu, listed in the file and the docs). `is_default: true` clears it on the user's other envs; on an archived env = 409.
  - Archive clears `is_default`. Unarchive does not restore default.
  - Validation (422 VALIDATION_ERROR): name 1–80 chars trimmed; network_access enum; hosts = hostnames or `*.` wildcards, lower-cased, de-duped, max 200; setup_script/env_vars ≤ 64 KB, no NUL; repos max 20, https/ssh/git@ only, branch no leading `-`, `..`, whitespace; secret name `^[A-Z_][A-Z0-9_]*$` ≤128; secret value 1 B–64 KB; ≤100 secrets/env. Unknown body fields rejected.
  - Secrets: reuse the existing platform AES-256-GCM `allternit_cloud_core::CredentialCipher` (`ALLTERNIT_CREDENTIALS_KEY`, already in deploy config — no new env var), split into nonce/ciphertext exactly like `services::inference_keys`. No cipher → secret routes and `/resolve` answer 503 `code_environment_secrets_not_configured`; CRUD still works.
  - `/resolve`: runtime device token only (Clerk/API token = 401), device owner must own the env (else 404). Returns the env flattened + `secret_values`. Archived envs still resolve. Logs only env id + secret count.
- `cmd/allternit-cloud-api/openapi/code-environments.yaml` (new file; not added to `platform-v1.yaml` because that file is the tested source of truth for the `/v1` project-key routes only) and a "Code Environments" section in `cmd/allternit-cloud-api/API.md`.
- Docs: user guide `surfaces/docs/code/cloud-environments.mdx` → **https://docs.allternit.com/code/cloud-environments** (new "Code mode" group in the Documentation tab, after "Workspace workflows"; there was no existing Code docs folder). Dev page `surfaces/docs/api/code-environments.mdx` → https://docs.allternit.com/api/code-environments (API Reference → Agents & Runtime). `check_links.py` 0 problems; `node scripts/docs-lint.cjs` passed.
- Dependency map: `feature:code-cloud-environments` added to `docs/dependency-map/features.json`. `--validate` reports one pre-existing, unrelated failure (`feature:interactive-answers` → `allternit-ai/src/lib/apps`, missing in the local allternit-ai checkout).

## For the consumers
- ENVUI: POST returns 201 (not 200). Label "Limited" matches. Docs link: `https://docs.allternit.com/code/cloud-environments` (anchor `#network-secrets` for the secrets help link).
- ENVRT: `GET {cloud}/api/v1/code-environments/:id/resolve` with the runtime device token; response is the CodeEnvironment fields at top level plus `secret_values`. 503 means the cloud has no cipher configured — surface as `failed`.
- PRLINK/ENVRT may also add pages under `surfaces/docs/code/`; the "Code mode" nav group in `docs.json` is the place for them (expect a trivial docs.json merge).

## Unfinished
- Nothing in scope. Not compiled (by rule) — first build may surface small type nits.
