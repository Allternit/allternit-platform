# Agent Gateway acceptance matrix: platform

Spec: `Allternit Brain/Research/specs/agent-gateway.md` (Acceptance) and `channel-packs.md` (Acceptance).
UI lines live in the web repo: `docs/gateway-acceptance.md` (allternit-ai).
Status: `verified (automated)` = TS test run in this pass; `verified (automated, Rust: run 2026-09-30)` = named Rust test exists in
`cmd/allternit-api`, not executed here (no cargo in this pass); `blocked: <reason>`.

Paths: SG = `services/subscription-gateway/test`, AG = `platform/packages/agent-gateway/test`,
FC = `platform/packages/subscription-fabric-contracts/test`, GR = `cmd/allternit-api/src/gateway_runner.rs`,
GP = `cmd/allternit-api/src/gateway_placement.rs`, GA = `cmd/allternit-api/src/gateway_acceptance.rs` (end-to-end walks), CG = `cmd/allternit-api/src/channel_gateway.rs`.

Rust run 2026-09-30 on `gateway/accept-rust` (cargo test -p allternit-api --lib, per filter): gateway 352 · a2a 2 · aai_facade 2 · channel 24 · teams 6 · discord 5 · mcp_server 4 · coordinator 14 · thread_routes 13 · slack 4, all pass; bins build. Every GR/GP/GA/CG line below marked `Rust: run 2026-09-30` is now run and passing.
New this pass: `SG/aai-conformance-route.test.ts` (ACR) + `adapters/<id>/fixtures/offline.ts` for 5 adapters.

## Gateway conformance (TS)

| Line | Test | Status |
|---|---|---|
| Every registered adapter passes `runConformance` via the AAI host conformance route with offline fixtures | ACR `<id> passes conformance via POST /aai/conformance/<id>` for grok-bot, claude-desktop, claude-managed-agents, chatgpt-dots, openclaw; guard test `every registering adapter ships adapters/<id>/fixtures/offline.ts` fails when a new adapter lacks fixtures | verified (automated) |
| Route contract (auth, 404 unknown, report shape) | SG/aai-host.test.ts `runs conformance for a registered provider, 404 otherwise` | verified (automated) |
| Harness itself detects broken providers | AG/broken-provider.test.ts `FAILS a provider that contaminates contexts, double-executes replays and auto-approves`; `defect ... fails only via ...` | verified (automated) |
| An Allternit Bot passes loopback conformance | AG/loopback.conformance.test.ts `passes every applicable area`; SG/aai-host.test.ts registers loopback in `createAaiHost` | verified (automated) |
| chatgpt-web lane conformance | SG/chatgpt-web-conformance.test.ts | verified (automated); note chatgpt-web has no `aai.ts` so it is not an AAI-registered adapter |

## Architecture phase

| Line | Test | Status |
|---|---|---|
| Every external worker fits a BotExecutionBinding; every unit of work fits a RemoteThreadBinding behind a Thread generation | FC/gateway.test.ts `roundtrips wire types`, `execution transitions`, `remote thread transitions`; GR `vendor_turn_opens_once_sends_once_and_bridges_events` | verified (automated) TS; GR verified (automated, Rust: run 2026-09-30) |
| Every connection path fits ProviderAccountBinding + ConnectionProfile | FC/gateway.test.ts `connection transitions`, `roundtrips profiles and manifests` | verified (automated) |
| Coordinator only asks generic questions | GP `missing_capability_and_unready_binding_move_the_step_with_reasons`, `keeps_planner_pick_when_it_qualifies` (placement uses capability manifest, no vendor branches) | verified (automated, Rust: run 2026-09-30) |
| Ledger distinguishes exact / best-effort / inferred | FC/gateway.test.ts `roundtrips capability manifest, error, event, approval, origin, memory, mirror`; AG/mirror.test.ts `unobservable ... is never claimed synced` | verified (automated) |
| Vendor and Allternit approvals never conflated | AG/router.test.ts `never auto-resolves an approval without a human actor`; aai-sdk client.test.ts `refuses to respond to approvals without human intent`; GR `allternit_approval_gates_consequential_sends_and_never_resolves_a_vendor_one` | verified (automated); GR (Rust: run 2026-09-30) |
| Thread can change runtime/vendor across generations without losing state | GR `handoff_closes_old_binding_and_opens_a_new_one` | verified (automated, Rust: run 2026-09-30) |

## First milestone (server-side halves)

| Line | Test | Status |
|---|---|---|
| Vendor accounts connect; agents appear as vendor-backed Bots with Verified | SG/accounts-hub.test.ts, SG/connect-endpoint.test.ts; web CC/LW tests | verified (automated) |
| Whole first milestone in order (connect, verify, discover, bind READY, isolated context, round-trip, second thread isolated, vendor approval a bot can't answer, context reset, new generation, revoke, NEEDS_AUTH, threads intact) | GA `first_milestone_walk` | verified (automated) |
| Thread opens isolated remote context; messages/events round-trip | GR `vendor_reply_lands_in_the_transcript_once_with_attribution`; SG/thread-mapping.test.ts; SG/events.test.ts | verified (automated); end-to-end with a real vendor account: blocked: needs Eoj vendor sign-in |
| Allternit bot messages vendor bot and gets reply | AG/loopback.conformance.test.ts `maps Thread <-> context...`; aai-host `dispatches ops through the router` | verified (automated) |
| Vendor approval appears as vendor-authority card | GR `vendor_approval_needs_you_and_only_a_human_answers_it`; web LW | verified (automated); GR (Rust: run 2026-09-30) |
| Thread survives remote context reset via new generation; no contamination | GR `lost_context_starts_a_new_generation_unless_resume_is_declared`, `rate_limit_is_respected_without_changing_state_or_mixing_contexts`; AG conformance `isolation` area | verified (automated); GR (Rust: run 2026-09-30) |
| Revoking account moves dependent Bots to Needs attention, keeps Threads | GR `each_failure_code_has_its_state_effect`, `api_key_account_without_a_key_fails_fast_to_needs_auth`; web CC kill-switch/disconnect | verified (automated); GR (Rust: run 2026-09-30) |
| ui_bridge lane terms warning before first use | manifests carry `termsWarning` (SG/grok-bot-adapter.test.ts `declares ui_bridge / best_effort / desktop_session`); UI gate in web GW | verified (automated) |
| PackGap recorded for unsupported vendor object | server route: aai-sdk client.test.ts `covers vendor packs, bindings, sync`; FC/gateway.test.ts `packParity`; UI half in web matrix | verified (automated) |
| Look profile cannot suppress the provenance layer | FC/gateway.test.ts `look profile has no provenance-suppression field and strips smuggled ones` | verified (automated) |
| Vendor memory reachable through Allternit memory | AG conformance `memory` area (declared adapters) covers the AAI op; no Allternit-memory bridge/UI | verified (automated): `GET /gateway/bots/:id/vendor-memory` + human-only promote to a native note with provenance, `agent_gateway_routes` `vendor_memory_*`, `promote_creates_one_native_note_with_provenance_and_one_event`. Every shipped adapter reports memory opaque, so the readable path has no live producer yet |
| Vendor artifacts in Allternit artifacts library | SG/artifacts.test.ts (gateway store); UI registration in web matrix | verified (automated) |

## Channel packs (server-side lines)

| Line | Test | Status |
|---|---|---|
| Real Slack/Teams messages < 3s; reply lands in right channel thread | none | blocked: needs a real Slack workspace and Teams tenant (spec open question) |
| Slack inbound opens a thread; outbound remote id stored; reconnect has no duplicates | GA `channel_milestone_slack_walk` | verified (automated) with a fake Slack; real workspace: blocked (see first row) |
| Disconnect/reconnect resumes at cursor with no duplicates | SG/outbox-replay.test.ts; SG/events.test.ts (cursor); AG router `returns first result for replayed correlationId` | verified (automated) for gateway cursor/replay; channel adapter live resume blocked: needs real workspace |
| Every outbound post logged; consequential needs approval | CG `every_outbound_outcome_writes_a_log_row_and_a_ledger_event` (sent, denied, approval-pending, rejected, unconfirmed, Slack reply path), `policy_denies_or_asks_and_an_approval_is_single_use`; GR `allternit_approval_gates_consequential_sends_and_never_resolves_a_vendor_one` | verified (automated) |
| Vendor agent via channel: vendor identity + transport | web CT `Muse via WhatsApp...`; `channel_transports` `muse_over_whatsapp_sends_and_returns_replies_as_agent_message_completed` | verified (automated) both repos |

## Defects found

None in the TS gateway. Gap closed: no registration exposed offline fixtures, so the conformance route could not be exercised per adapter; added `adapters/<id>/fixtures/offline.ts` (grok-bot, claude-desktop, claude-managed-agents, chatgpt-dots, openclaw) and a guard test.
