# Unified channel API (Phases 1–4, offline-verified)

- Session: Claude Code (Opus 5.5), 2026-10-13/14, for Eoj
- PRs: platform #1524 (merge fc708983748bdab764c02bbf9d8e0d7491d2ab3d), allternit-ai #543 (merge 126ff63a6a15b26361e620f490200be04f6ee5c7)
- Plan: Brain `Research/specs/allternit-unified-channel-api.md`; Factory dag_818173

## Done
- Extended the existing `ChannelTransport`/`channel_gateway` (no new gateway, no AgentChat dependency, no migration). Audit, contract and decisions: `docs/channels/UNIFIED_CHANNEL_API.md`. API reference: `surfaces/docs/guides/channel-gateway-api.mdx`.
- Fixed cross-owner/cross-connection conversation routing. Telegram DMs keyed by the person's id let one owner's message land in another owner's thread and bot. Also fixed the shared Slack app refusing every owner but the first for a team.
- Inbound recorded before the platform or cloud queue is answered; durable bot turns (queued/running/done) serialized per session; startup recovery (queued turns run, cut-off turns marked interrupted with a chat notice, never re-run).
- RateLimited outcome (429 + Retry-After, retryable by correlation id). Slack's header-only wait carried through the runtime and the cloud send route. Pending sends become unconfirmed at startup and are reconciled from platform history.
- Per-adapter capabilities; two thread routes (channel info, delivery status); contract tests across 9 adapters. These found WhatsApp Business / Discord app 429 and 503 bugs, now fixed. The Discord speaker lookup is scoped to the server.
- UI (shared message card, so Desktop + ai.allternit.com phone + PWA): "Rate limited, not sent" state, hints on every delivery status, 429 message on file sends.
- Found and fixed while smoke-booting the merged tree: main's allternit-api panicked at startup. `0a6b6a0327` had added a duplicate `/v1/descent` + `/v1/decisions/*` at the root via `kernel_alias_router`; the root mount now takes only the run aliases.

## Verification
- allternit-api `--lib channel` 173 passed; `kernel_ui` 15; `agency_api` 148; cloud-api `slack_app` 11. `cargo build --bin allternit-api` ok, and a smoke boot on a fresh db gave health 200 with every new and affected route resolving (401 unauthenticated).
- allternit-ai typecheck clean; gateway vitest 491/492. The one failure is the unrelated `vendor-identity.test.tsx` (vendor logo asset another session is changing).
- Docs `check_links.py` 0 problems.

## Deferred (honest)
- Live provider tests: not run (they send real messages and need authorized test accounts). Plan for another session: `docs/channels/LIVE_TESTS.md`.
- Desktop runtime publish and the m.allternit.com PWA deploy were not done; both await Eoj's go-ahead.
- Dependency-map `--validate` reports a pre-existing error on `feature:interactive-answers` (unknown component `allternit-ai/src/lib/apps`). Not touched.
