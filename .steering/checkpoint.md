# Steering checkpoint

## Goal
Unified channel API, Phase 1 audit + Phase 2 Slack slice (Brain spec Research/specs/allternit-unified-channel-api.md; Factory dag_818173). Extend the existing ChannelTransport/channel_gateway; no new gateway, no AgentChat dependency.

## Just did
Audit (docs/channels/UNIFIED_CHANNEL_API.md). Fixed cross-owner/cross-connection conversation routing (find_binding_for scoped to owner+account; ensure_binding_on, route_inbound, channel_start, lane, people use it; Slack team lookup scoped to the relayed owner). Added Capabilities per adapter, PostError::RateLimited/SendOutcome::RateLimited (429 + Retry-After, retryable by correlation id), fetch_since default = history_unsupported, delivery_status + GET thread channel/channel-messages routes, startup recover_interrupted_sends. Dev doc channel-gateway-api.mdx + user status section + nav + dependency map.

## Next
cargo test -p allternit-api channel tests; fix failures; report to Eoj. No commit/push/PR/deploy until Eoj says so.

## Open questions
Inbound record-before-ack split (all providers share dispatch_events_with_files) deferred to Phase 3. Live provider tests need authorized test accounts.
## Vendor bot coverage — 2026-10-14
DAG dag_122853 / node n_4731 / WIH wih_2875 remains in progress. Reused backend session/avatar-glass-docs-20261011 and frontend session/project-banner-20261011 per Eoj authorization. Seven native adapters committed/pushed (OpenMausBot, Codync, Hivekeep, Invisible Dots, CopilotKit OpenDots, DS Bot, CopilotKit OpenMuse); all33icons and credential profiles shipped in branch. Backend head1730da9c1a, frontendc8216995. Focused DS/OpenMuse strictchecks pass, OpenMaus/Codync13checks pass, frontendcredential5checks pass, docs links0problems, dependency map valid. No liveaccount certification, merge or deployment yet. Cheap in-process Luna helpers now implement Nightly OpenBot and investigate Rakazo in distinct adapter folders. Remaining connectors unfinished; do notclaimfull33working.

## Vendor bot coverage — 2026-10-14 (Claude, finishing Codex's run)
All 33 pictured projects settled. 13 native adapters (adds Nightly OpenBot, Rakazo, FrockBot, Sidemates, nanoMuse, Vellum Assistant); Grok Bot / OpenAI Dots / Muse via existing packs; 17 marked "No supported connection" with reasons (hosted no-API, session-only, E2E-encrypted, keystroke-driven, unauthenticated). Adapter typechecks clean; UI typecheck + 269 gateway tests pass; docs 0 link problems. No live vendor-account verification, merge or deploy yet.
