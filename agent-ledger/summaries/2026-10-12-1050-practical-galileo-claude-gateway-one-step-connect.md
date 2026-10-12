# Agent Work Attestation — Gateway one-step connect

**Date:** 2026-10-12 10:50  
**Session ID:** practical-galileo-c8wxvt (Claude Code cloud session)  
**Branch:** claude/practical-galileo-c8wxvt  
**Agent:** claude  
**Commit:** platform #1513, allternit-ai #534  
**Ledger entry:** [../LEDGER.md](../LEDGER.md)

## What was done

- Gap analysis of bots, the Agent Gateway (AAI) and the managed-agent surface against bot platforms, ChatGPT dots and Claude Managed Agents. Shared with the owner, not committed.
- `cmd/allternit-api/src/gateway_connect.rs`: `GET/POST /api/v1/gateway/connect`, one idempotent call from a product (chatgpt, chatgpt-dots, claude, claude-managed-agents, gemini, copilot, kimi, grok-bot, openclaw, hermes, muse) to READY bots.
- `agent_routes.rs`: `create_agent_json` (in-process `POST /agents`). `agent_gateway_routes.rs`: `ApiErr::message`, `AUTH_TYPES` pub(crate), mounts the routes.
- allternit-ai: Quick connect panel (`QuickConnect.tsx`) above the step-by-step wizard, client calls in `lib/gateway/api.ts`.
- Docs: QUICKSTART one-step section and the previously undocumented `ALLTERNIT_LOCAL_SUBS_GATEWAY` single-machine switch; AAI_REST routes; README `protoc` prerequisite; dependency map `feature:vendor-subscription-bots` (building).

## How it works

- Chains existing pieces only: the gateway's `/v1/accounts` login flow via `subscription_routes::forward`, `discover_agents`, an `agent.health` probe, `apply_account_state` along `connection_path`, `upsert_exec_binding` and `set_exec_state`.
- The account reaches CONNECTED, and bindings READY, only after discovery and health answered.
- Browser logins are linked as `externalAccountId = subsfab:<login>`, so the existing subscription sync keeps them in step.
- The person always does the login themselves in a plain window. Nothing automates credentials or clears verification checks.

## Verification

- `cargo build -p allternit-api` OK. `cargo test -p allternit-api --lib gateway_connect`: 2/2 pass.
- Live in a container (API in local-gateway mode, subscription gateway, fake OpenClaw):
  - openclaw: ready, and a thread turn through the created bot answered with 8 events.
  - muse: ready.
  - claude-managed-agents: needs_key (Anthropic rejected a test key on a real round trip).
  - grok-bot, chatgpt, claude, dots, hermes: needs_setup with the right reason.
  - Gateway down: needs_setup.
  - Unknown product: 400.
  - Re-running connect is idempotent.
- allternit-ai: quick-connect tests 7/7. Gateway suites 487/488; the one failure (`vendor-identity` fighter logo) also fails on main. tsc has no new errors (4 existing errors in IdentityStep.tsx). eslint clean.
- `scripts/dependency-map.py --validate` OK.

## Known gaps / remaining work

- No live vendor session was run: the container has no Chrome, no Grok app and no vendor accounts.
- The ChatGPT, Claude, Gemini, Copilot, Kimi and dots web lanes remain offline-verified only; expect selector fixes on the first live run.
- The Quick connect panel was not clicked through in a browser.
- Not done (from the gap analysis): a worker for `/beta/work` (managed sessions never execute), Managed Agents streaming and `user.custom_tool_result`, AAI SSE, persisted AAI contexts, per-adapter load isolation in `registerVendorAdapters`, the `runtime-events-contract` notify-URL test regression.
- Lifecycle steps 8–9 (desktop rebuild, worktree cleanup, `git-discipline-check.sh`) apply to the owner's machine and were not run from this cloud session.

## Files changed

- cmd/allternit-api/src/gateway_connect.rs (new), agent_gateway_routes.rs, agent_routes.rs, lib.rs
- docs/gateway/QUICKSTART.md, docs/gateway/AAI_REST.md, README.md, docs/dependency-map/features.json
- allternit-ai: src/components/gateway/QuickConnect.tsx (new), __tests__/quick-connect.test.tsx (new), GatewayLibrary.tsx, src/lib/gateway/api.ts
