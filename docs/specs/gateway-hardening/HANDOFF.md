# HANDOFF — Gateway Hardening + Console Control Plane (session/gwrobust)

> For the next agent picking this up. You have zero prior context; everything you need is here.
> Read `.steering/checkpoint.md` and this file first. Spec: `docs/specs/gateway-hardening/plan.md` (in this worktree).

## Goal

Close the P0 gaps from the gateway gap analysis (`Allternit Brain/Products/GatewayGapAnalysis.md`, written 2026-09-19 from a code inventory of the LLM gateway + an industry survey of OpenRouter / LiteLLM / Portkey / Cloudflare AI Gateway / Kong / Envoy AI Gateway). Overall grade was **B**; the laggards were failover & health (C), testing (C), cross-cutting stubs (C). Then ship the first console control-plane pages.

Scope this session: **P0.1–P0.4 + two console pages.** P1/P2 items stay in the brain doc — do not expand scope.

## Worktree & environment (non-negotiable)

- Worktree: `~/Desktop/allternit-workspace/allternit-session-gwrobust`, branch `session/gwrobust`, based on origin/main @ 1a3664a21 (2026-09-19).
- FIRST ACTION: `git fetch origin` and check `HEAD..origin/main`. If main moved, rebase/merge forward before doing anything else (repo commandment: stale checkouts lie).
- `export CARGO_TARGET_DIR="$HOME/Desktop/allternit-workspace/.shared-target"` before every cargo command. Never `cargo clean` it.
- **NEVER set `ALLTERNIT_API_PORT=8013`** — that port belongs to the installed Desktop app; use 18013 with a scratch `ALLTERNIT_DATA_DIR` for any live boot.
- pnpm only, never npm install (console surface).
- Repo process rules are in the root `AGENTS.md`: session worktree mandatory, a Factory workspace DAG required for multi-step work, full lifecycle (commit → push → PR → merge → sync main → ledger attestation → cleanup → `scripts/git-discipline-check.sh` PASS) before the session counts as done.
- Multi-step work must be registered in a Factory workspace DAG: `gizzi workspace plan new "<goal>"` (or `allternit-factory workspace plan new`). If neither is on PATH, build with `cargo build -p allternit-factory --release` (shared target) and run the binary from the target dir.

## Current state (paused 2026-09-19)

### ✅ Done and verified — console pages (uncommitted, safe)

In `surfaces/platform.allternit.com/`:

- `src/lib/gateway-routing-policy.ts` — typed client for the provider-routing admin endpoints.
- `src/pages/console/gateway/RoutingPolicyPage.tsx` — policy editor (Hermes six flat keys + per-model overrides table) with a prominent **live resolve-preview widget** (POST /gateway/provider-routing/resolve) and Hermes YAML export button.
- `src/pages/console/gateway/RouteCredentialsPage.tsx` — BYOK credential manager (masked fingerprints, add w/ validate-before-store, inline delete confirm, zero-cost-metering helper text).
- `src/App.tsx` — two routes (`/gateway/routing-policy`, `/gateway/credentials`) in ConsoleRoute.
- `src/components/console-ui/navConfig.ts` — "Gateway" nav group (also indexes into ⌘K palette).

Verified by the authoring agent: `npx tsc --noEmit` clean, `pnpm build` green, no new deps. Known caveat (documented in the page): GET provider-routing returns only the caller's own tenant row — a null policy does NOT mean no policy applies, because the platform-global (NULL-tenant) row may; the page says this honestly.

### ⚠️ Partial — Rust P0.1 + P0.4 edits (uncommitted, UNVERIFIED)

Dirty files in `cmd/allternit-api/src/llm_gateway/`: `failover.rs`, `embeddings.rs`, `images.rs`, `realtime_audio.rs`, `translate.rs`, `metrics.rs`. These are edits from an agent cancelled mid-run. They are intended to be:

- **P0.4 fence the fakes:** embeddings/images/realtime return 501 `allternit.not_configured` when no provider is configured (escape hatch env `ALLTERNIT_GATEWAY_ALLOW_FAKE_PROVIDERS=1`).
- **P0.1 cooldown tracker:** health-based failover in `failover.rs` — per-(provider,model) in-memory cooldowns (429 → 5s, >50% fail rate in 60s → 30s, configurable via RetryPolicy with serde defaults), fail-open when all cooling, Prometheus counters, wired into the proxy retry loop.

Do NOT trust or commit these until: (1) you review the diff, (2) `cargo test -p allternit-api llm_gateway` is green, (3) the behavior matches the spec section below. Finish/fix as needed, add tests if missing, then commit.

### ❌ Not started

- **P0.3 wire conformance harness** — new mock-provider fixture server + cases, all localhost, in `cargo test`. NOT a live smoke test (owner decision removed those; fixtures in CI don't conflict with that). Cases: non-stream happy path, SSE stream, 429→failover, 500→retry, validation error shape, auth 401. The upstream the gateway proxies to is the **gizzi runtime** (`cmd/gizzi-code`, default http://127.0.0.1:4096) — the mock must mirror what the gateway expects from gizzi, which is the tricky part; read `proxy.rs` + `gizzi_bus.rs` carefully first. If full in-process boot is impractical, test proxy handlers directly with test state and document the choice.
- **P0.2 streaming failover** — currently streams get ONE attempt (retry loop is non-streaming only, proxy.rs ~2097-2118). Spec decision: implement a structured `allternit.retry_hint` SSE event on upstream stream failure (`{retryable, reason, next_fallback}`) for gizzi to re-drive; gateway-owned transparent resume is deferred.
- Factory workspace DAG registration for the remaining nodes.
- Entire landing lifecycle: push, PR (merge commit), sync shared checkout main, ledger attestation in `agent-ledger/summaries/` + `LEDGER.md` (STEER_GUARD_OFF=1 for the ledger commit on main), worktree cleanup, `scripts/git-discipline-check.sh` PASS output pasted in the summary.
- Checkpoint steering: update `.steering/checkpoint.md` at milestones (Goal/Just did/Next/Open questions).

## Spec summary (the contract the Rust work must meet)

Full spec in `docs/specs/gateway-hardening/plan.md`. Key points:

- **P0.1** cooldowns in-memory deliberately (shared state is a later phase); fail-open, never hard-fail when all providers cooling; time must be injectable for tests (no sleeping).
- **P0.4** never silent fake output on billed surfaces; 501 + stable code; env escape documented in module headers; tests assert 501 when env unset.
- **P0.3** pure CI fixtures, no external network; asserts on the gateway's external OpenAI-compatible contract.
- **P0.2** hint-event path only; pick the owning layer deliberately and document.
- Verification contract: `cargo test -p allternit-api llm_gateway` green; `npx tsc --noEmit` clean in the console; live boot on 18013 to smoke the 501s and the routing resolve endpoint; ledger attestation at the end.

## Key file map

- Gateway: `cmd/allternit-api/src/llm_gateway/` — `proxy.rs` (~3.4k LOC, chat_completions ~1640-2305, retry loop ~2097-2118, BYOK attach/strip ~1863-1875/2235-2249, usage recording ~984-1135), `failover.rs`, `provider_routing.rs`, `route_credentials.rs`, `admin_routes.rs` (gateway admin router ~30-68; provider-routing handlers ~1017-1176), `embeddings.rs`, `images.rs`, `realtime_audio.rs`, `translate.rs` (error codes ~20-36).
- Console: `surfaces/platform.allternit.com/` — pages under `src/pages/console/`, clients under `src/lib/`.
- Brain: `Allternit Brain/Products/GatewayGapAnalysis.md` (the grades this work is closing).

## Open questions

- P0.2: hint-event (spec's choice) vs gateway-owned resume — implement hint path; revisit only if gizzi re-drive proves insufficient.
- Whether P0.1's `RetryPolicy` new fields need a migration (serde defaults were the plan — verify old rows deserialize).
