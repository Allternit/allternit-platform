# Agent Work Attestation — Vendor-bot gap closure

**Date:** 2026-10-13 23:30  
**Session ID:** practical-galileo-c8wxvt (Claude Code cloud session)  
**Branch:** claude/gateway-integrate (merges claude/gateway-core, claude/ma-adapter, claude/connect-consent, claude/session-worker, claude/bot-safety); UI: allternit-ai claude/connect-consent  
**Agent:** claude (coordinator plus three subagents)  
**Ledger entry:** [../LEDGER.md](../LEDGER.md)

## What was done

These close the gaps listed in the 2026-10-12 attestation (gateway one-step connect).

- **Consent and tool results.** One-step connect asks for the person's consent once per account before it drives a vendor's website or app (`needs_consent`, `acceptTerms`, recorded in `connection_audit`). The Quick connect panel shows the terms. `POST /threads/:id/gateway/tool-result` answers a vendor's custom tool call; `needs_attention` puts the thread in `needs_you`.
- **Live check script.** `scripts/e2e/gateway-live-check.sh` is the owner-run live checklist: connect, consent, sign-in, two test turns per product, and a report.
- **Subscription gateway.**
  - Per-adapter load isolation.
  - AAI contexts persisted in SQLite (migration 0009). openclaw and hermes re-adopt them through a shared OpenAI-compatible client; UI-bridge adapters report the context as lost cleanly.
  - `GET /aai/events/stream` (SSE).
  - A per-account FIFO turn queue for website and desktop subscriptions, with a "queued … position N" status, plus `RATE_LIMITED` / `CONTEXT_BUSY`. The router now counts in-flight contexts only.
  - The notify-URL test now expects `/api/factory`.
- **Claude Managed Agents adapter.**
  - Events come from the SSE stream, with cursor backfill and reconnect.
  - `agent.custom_tool_use` becomes `user.custom_tool_result`, through a new optional AAI `MessageInput.toolResult`.
  - Session listing and adoption; a clear error for a bad environment id.
  - A README.
- **allternit-api `session_worker.rs`.** Executes queued `/beta/work` session and deployment tasks as Gizzi turns: lease, heartbeat, events, interrupt, ack, and a retryable failure when Gizzi is down. `ALLTERNIT_SESSION_WORKER=0` disables it.
- **gizzi-code bot policy.** Bots with no allowlist run in `default` mode: read-only tools are pre-approved, and shell, writes and web fetches go to the app's approval flow. `GIZZI_BOT_BYPASS_PERMISSIONS=1` is the escape hatch.

## Verification

- **allternit-api:** `cargo test -p allternit-api --lib`
  - gateway_connect + gateway_runner: 26/26.
  - session_worker + beta_* + cloud_agents: 48/48.
  - Combined run on the integration branch: see the PR.
- **subscription-gateway (combined branch):** 502 passed, 1 failed. The failure is the macOS-only xattr test; 4 suites need Chrome and do not run here. agent-gateway: 28/28. Typecheck clean.
- **gizzi-code bot-turn:** 15/15. The neighbouring suites show the same 4 failures as main (missing gizzi-sdk build artifacts).
- **allternit-ai quick-connect:** 8/8, eslint clean.
- **dependency-map `--validate`:** OK.

## Known gaps / remaining work

- No live vendor session has been run. The owner runs `scripts/e2e/gateway-live-check.sh` on a Mac with real accounts; expect selector fixes for the ChatGPT, Claude, Gemini, Copilot, Kimi and dots web lanes.
- Muse needs a WhatsApp Business number and a Meta app.
- The gizzi-code change needs a gizzi-code release through npm, Homebrew and the Desktop sidecar, per the one-current-build rule. This was not done from the cloud session.
- A context open still waiting in the queue cannot show the queued status yet.
- Pacing, kill-switch and dedupe counters are still in memory only.
- AAI artifacts have no upload shape, so Managed Agents file upload is skipped.
- Lifecycle steps 8–9 (desktop rebuild, `git-discipline-check.sh`) apply to the owner's machine.
