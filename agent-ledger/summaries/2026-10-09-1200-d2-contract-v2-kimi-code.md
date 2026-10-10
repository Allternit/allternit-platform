# D2 contract v2 — structured members, executor, credentials, Windows transport (kimi-code)

Date: 2026-10-09 (morning–midday CDT). Agent: Kimi Code (D2 build+verify session). Branch `feat/computer-contract-v2`, platform **PR #1451**, one commit (`8672fa9096` + tree-restore fixup `156ace0ad7`).

## What landed

Phase D2 of `SPEC-allternit-driver-fast-path-2026-10-09.md` on top of merged D1 (#1445/#1446):

- **Contract `allternit.computer.v2`** (`contracts/computer-toolset/allternit-computer-v2.json`): the 17 v1 pixel members byte-identical + 6 structured members (`read_ui`, `act`, `run_batch`, `verify`, `request_human`, `use_credential`). allternit-api serves it as the `computer` toolset (v1 calls stay valid); `generate.mjs` emits `COMPUTER_V2` bindings (TS `CONTRACTS.computer` stays v1 for the Claude-native configs; Python keys at the v2 superset).
- **Executor** (`computer_v2.rs`): every new member runs the existing lease → policy → approval → audit pipeline; `act` text-entry ops (set_value/select/press) and `use_credential` upgrade to human approval on non-sandbox targets (same rule as `type`/`key`); `request_human` releases the guest lease, emits `computer.human_requested`/`human_resumed` (live view, input enabled) and resumes on `POST /api/v1/computers/:id/human-done` or timeout; structured reads act through the sidecar with no screenshots.
- **Credentials**: sealed vault (+ optional app/domain `bind`; routes fixed to the documented `/api/aci/*` prefix), macOS Keychain, Bitwarden/1Password CLI adapters (interface-only here — CLIs absent), RFC 6238 TOTP (reused `aci_credentials::totp_code`). Values never enter the model context/logs/audit; verified by outcome + grep.
- **Windows transport**: this-device input off the Cua CLI fallback onto the sidecar over token-gated loopback TCP (`ALLTERNIT_DRIVER_ENDPOINT`/`TOKEN`, endpoint-file support).
- **Adapters**: gizzi `computer_v2` function tool for every family with read_ui/run_batch steering guidance.
- **Driver fixes found live**: Cua MCP session revival (dead daemon-side session took every call down until restart); element `press` rerouted off arc's SkyLight key-posting path — it SIGSEGV'd the sidecar (exit 139, twice, reproducibly on macOS 14) — onto the Cua engine with contract-key normalization.

## Verification (this Mac only, per gate)

- `cargo test -p allternit-api --lib computer` **128 passed** (123 pre-D2 + 5 new); `cargo check` clean; driver unittest **12/12**; plan-schema python test passed; gizzi `bun run typecheck` clean; docs link check **0**; dep-map `--validate` exit 0 (pre-existing `feature:artifacts` warning).
- Smoke-boot allternit-api (worktree debug build, port 18013, scratch data dir, `ALLTERNIT_LOCAL_DEV_BYPASS=1`, sidecar at `/tmp/allternit-d2.sock`) — served every verification call below for ~1.5 h.
- Live per member through the executor: `read_ui` Calculator 92.5 ms cold; `act` click 356 ms; `verify` ok via verify_state; `run_batch` clear/7/+/8/= + expect 15 → 5/5 ok, 2.18 s, cross-check ok; approval round trip (409 → approve → single-use grant); `request_human` + `human-done` resume; schema reports v2 + per-member availability + credential backends.
- `use_credential`: domain-mismatch refusal; TOTP generated from the sealed seed and typed into the Calculator display; `verify` confirmed the value (outcome); secret appears in no log or audit row (API log, sidecar log, driver audit, policy audit).
- **Per-family real tasks** (Calculator 7+8=15 through `computer_v2`): **Gemini 2.5 Flash — completed end-to-end** (read_ui → run_batch → verify ok:true, 26 s, trace in the policy audit). OpenAI gpt-5 reached the harness and planned correctly but ended without tool calls; gpt-4o provider-errored; Claude sonnet-4/4.5 rejected by the OpenRouter key (≈$0.71 of $10 lifetime credit left — pre-flight reservation rejects gizzi's full prompt); ollama/omlx local lanes errored "Unknown" in the headless env. 1 of 4 verified live; the rest recorded honestly in PR #1451 + the spec Progress.

## Incidents + lessons

- The arc `press` segfault (native SkyLight path) — worked around by routing press to Cua; root cause still open.
- Two silent sidecar deaths during verification: one SIGSEGV (above), one SIGTERM of unknown origin; a watchdog loop (`/tmp/d2-watchdog.sh`) kept it alive for the model runs. The Desktop-managed sidecar in production doesn't have this exposure pattern (long-lived, parented), but a watchdog/restart policy is worth considering for D9.
- `git checkout <old-commit> -- .` to resolve a rebase clobbered 37 upstream files changed on main since the branch point; caught by the symmetric diff stat and reverted (`156ace0ad7`). Check `git diff main...HEAD --stat` symmetry after any tree-level restore.
- `gh pr create` from a fork-tracked branch silently picks the fork head; push to the upstream remote and pass `--head Allternit:<branch>`. A repo hook runs a full cargo build behind `gh pr create`/`gh pr merge` in this tree — expect minutes, not seconds.

## Deferred / open

- Cloud-guest v2 members wait for the guest driver image (D1b); no guest reachable, no provisioning authorized (unchanged from D1).
- Bitwarden/1Password adapters untested (CLIs not installed).
- Remaining family verifications need OpenRouter credit or the local provider plumbing fixed.
- arc press segfault root cause.
