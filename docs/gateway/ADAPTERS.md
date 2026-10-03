# Writing and shipping adapters

**What this is:** how a vendor adapter plugs into the AAI host, plus the status of every adapter that exists.
**Who it's for:** engineers adding or maintaining an adapter.
**Last verified against:** platform commit `c3af0730ca`.

Related: [ARCHITECTURE.md](ARCHITECTURE.md), [CONNECTIONS_AND_CREDENTIALS.md](CONNECTIONS_AND_CREDENTIALS.md), [LOOK_PACKS.md](LOOK_PACKS.md).

## Directory layout

An adapter lives in `services/subscription-gateway/adapters/<id>/`. The files below are what the shipped AAI adapters share. Not every adapter has every file.

| File | Purpose |
|---|---|
| `manifest.ts` | The capability manifest (`AgentCapabilityManifest`) and the subsfab `AdapterManifest` fields (`adapter_version`, `plans`, `capabilities`, `selectors_version`) plus `termsWarning` for `ui_bridge` lanes. |
| `provider.ts` | The `AaiProvider` implementation. |
| `index.ts` | Public entry: exports the manifest and a factory (for example `openclaw = { adapterId, manifest, create }`). |
| `aai.ts` | Registration: exports `createAaiRegistration(env)`, returning `{ provider, pacing?, fixtures?, disabled? }`. Loaded at boot. |
| `driver.ts`, `cdp-driver.ts`, `replay-driver.ts`, `browser-driver.ts` | UI-bridge only. `driver.ts` is the seam, the others are the live and offline implementations. |
| `selectors.ts`, `observe.ts`, `minidom.ts` | UI-bridge only. Selector pack with `SELECTORS_VERSION`, page-to-state observer, dependency-free DOM engine so live and fixture pages share one code path. |
| `look-profile.json` | Data for the web look pack. See [LOOK_PACKS.md](LOOK_PACKS.md). |
| `fixtures/offline.ts` | Exports `createOfflineAaiRegistration()`: a registration wired to fixtures or a fake server, used by `POST /aai/conformance/<id>` in tests and by `POST /aai/conformance/<id>?offline=1` on a running gateway. |
| `README.md` | Status, recon findings, consent steps. Say what is verified live and what is inferred. |

Shared helpers: `adapters/_shared/ax/` (macOS Accessibility transport) and `services/subscription-gateway/native/ax-bridge` (Swift helper).

## The provider

`AaiProvider` (`platform/packages/agent-gateway/src/provider.ts`, types in `types.ts`) has one method per AAI operation: `list, get, capabilities, identity, contextOpen, contextMessage, contextSteer, contextCancel, contextClose, events, tasks, memory, computer, artifacts, approvals, snapshot, sync, health`. Extend `BaseAaiProvider` so unsupported operations return `UNSUPPORTED` instead of throwing.

Rules the harness and the host enforce:

- Return `AaiResult` (`ok` or `fail(code, message, {retryable, retryAfterMs})`). Do not throw. The host wraps thrown errors as `UNKNOWN`.
- Use only the AAI error codes: `UNSUPPORTED, AUTH_REQUIRED, AUTH_REVOKED, RATE_LIMITED, VENDOR_UNAVAILABLE, LANE_BLOCKED, CONTEXT_NOT_FOUND, CONTEXT_BUSY, APPROVAL_REQUIRED, POLICY_DENIED, ADAPTER_DRIFT, SYNC_CONFLICT, UNKNOWN`.
- Declare `context.maxParallel` and `context.isolation` truthfully. Placement relies on them.
- Never auto-approve a vendor prompt. `agent.approvals` respond is reachable only with human intent.
- `contextMessage` takes a correlation id. A replay must not send twice. The router returns the first result for a replayed correlation id.
- A bot-check, verification banner or unusual-activity page returns `LANE_BLOCKED` and latches. Do not retry, do not solve it.
- Selector drift returns `ADAPTER_DRIFT` and latches until the adapter is reset (`clearHalt()` in the UI-bridge adapters).

## Registration (`aai.ts`)

`registerVendorAdapters` in `src/aai/registry.ts` scans `adapters/*/aai.ts` (or `aai.js`) at boot and calls `createAaiRegistration(env)`. A module without that export makes boot fail with a clear error. Until an adapter finishes loading, calls for it return `UNSUPPORTED`.

```ts
// adapters/grok-bot/aai.ts (shape)
export function createAaiRegistration(env: NodeJS.ProcessEnv) {
  const port = Number(env.SUBS_GATEWAY_GROK_BOT_CDP_PORT ?? GROK_BOT_CDP_PORT) // 9231;
  return {
    provider: grokBot.create({ cdpPort: port }),
    pacing: { minGapMs: PACING.min_task_gap_s * 1000, maxPerHour: PACING.max_tasks_per_hour },
  };
}
```

`guardProvider` wraps every registered provider before the router sees it:

- **Kill switch:** a disabled adapter answers `LANE_BLOCKED` for everything except `list, get, capabilities, identity, health, events, contextCancel, contextClose`. Set at boot with `SUBS_GATEWAY_AAI_DISABLED` (comma-separated adapter ids) or `disabled: true` in the registration.
- **Pacing:** `contextOpen` and `contextMessage` respect `minGapMs` and a sliding one-hour `maxPerHour`. Exceeding either returns retryable `LANE_BLOCKED` with `retryAfterMs`. Wrapping before the router means idempotent replays do not spend pacing budget.

Host routes (`src/http/routes_aai.ts`): `POST /aai/call` (scope `tasks:submit`), `GET /aai/providers` (`tasks:read`), `POST /aai/conformance/:adapterId` (`tasks:submit`; `?offline=1` runs the adapter's `fixtures/offline.ts` on a throwaway provider instead of the live one, 404 `no_offline_fixtures` if it ships none).

## Fixtures, offline mode and conformance

Every registering adapter ships `fixtures/offline.ts`. A guard test fails if one does not. The conformance route runs the raw provider (not the guarded one) so the harness never spends pacing or trips the kill switch.

`runConformance(provider, fixtures)` runs 13 areas: `identity, context, isolation, parallelism, memory, events, approvals, computer, failure, idempotency, cancellation, sync, resources`. An area is skipped as unsupported when the manifest says the capability is absent. `ConformanceFixtures` carries `agentId`, optional contamination `tokens`, `approvalId`, `faulty` (builders for a provider whose vendor is failing in a given way), `remoteMutation` (to trigger drift) and `settleMs`.

The harness itself is tested against a deliberately broken provider (`AG/broken-provider.test.ts`). UI-bridge adapters run against recorded pages in `fixtures/`, API adapters against a fake client or fake server (`claude-managed-agents/fixtures/fake-client.ts`, `openclaw/fixtures/fake-server.ts`). Recorded-session replay lives in `platform/packages/agent-gateway/src/replay.ts`.

Passing conformance offline proves the adapter obeys AAI. It does not prove the vendor still behaves the way the fixtures say. That needs the live checklist in [OPERATIONS.md](OPERATIONS.md#live-verification-checklist).

## Selectors and versioning

UI-bridge adapters keep an ordered fallback list of selectors per named key in `selectors.ts`, marked `critical` or not. Bump `SELECTORS_VERSION` when the live UI drifts, and record in the file which selectors are verified in the bundle, verified live, or inferred. The manifest carries `selectors_version`. API and local adapters use `n/a-official-api` or `n/a-local-http`. The web look pack and the README should say the same thing the selector comments say.

## Transports

| Transport | Used by | Notes |
|---|---|---|
| Official API | `claude-managed-agents` | User's own API key, per-call credential. `lane: official`, `exact`. |
| CDP | `grok-bot`, `claude-desktop` (default) | Loopback-only. Launching the vendor app with a debug port is gated on explicit consent and never kills a running instance (returns `LANE_BLOCKED` asking the user to quit it). |
| macOS AX | `claude-desktop`, `chatgpt-dots` (opt in) | `native/ax-bridge` Swift helper. Needs the user to grant Accessibility in System Settings. Without the per-app consent flag every call answers `LANE_BLOCKED` and the helper is never spawned. See `adapters/_shared/ax/README.md`. |
| Browser (Playwright) | `chatgpt-dots` (default) | Own Chrome profile directory; nothing opens without consent flags. |
| Local HTTP | `openclaw` | The user's own OpenClaw gateway, OpenAI-compatible endpoint. |
| Channel | Muse (no TS adapter) | Handled in Rust, `ChannelLaneTransport`. See [CHANNELS.md](CHANNELS.md#muse-lane). |

## Credential handling

- `POST /aai/call` carries an optional top-level `credential: { apiKey }` next to `binding`. `parseCredential` drops anything malformed.
- The credential lives in `AsyncLocalStorage` (`src/aai/call-scope.ts`) for one call. It is never persisted, logged, or added to an event or error.
- Adapters read it through a global symbol (`adapters/claude-managed-agents/credential.ts`), so `src/` and `adapters/` share no imports.
- allternit-api unseals the key just now and sends it with the call. If an `api_key` account has no usable sealed key, the call fails fast with `AUTH_REQUIRED`.
- UI-bridge and local adapters never read a vendor login, cookie or token. The user signs in inside the vendor app.

Details in [CONNECTIONS_AND_CREDENTIALS.md](CONNECTIONS_AND_CREDENTIALS.md).

## Adding an adapter, in order

1. Write `manifest.ts` (lane, guarantee, `context`, `messaging`, `approvals`, `events`, terms warning) and check it against `agentCapabilityManifestSchema`.
2. Write `provider.ts` extending `BaseAaiProvider`. Start with `list`, `capabilities`, `contextOpen`, `contextMessage`, `events`, `health`.
3. Add `aai.ts` with `createAaiRegistration(env)`, pacing, and any env vars (prefix `SUBS_GATEWAY_`, document them in [OPERATIONS.md](OPERATIONS.md#environment-variables)).
4. Add `fixtures/offline.ts` and make `POST /aai/conformance/<id>` pass.
5. Add `look-profile.json` and the web look pack. An adapter is not done until its look pack renders (spec build order rule).
6. Write the README: what is verified live, what is inferred, and the consent steps for a live run.
7. Add a row to the table below.

## Shipped adapters

"Verified live" means a real vendor session ran with the account owner's consent and is recorded in the adapter README.

| Adapter id | Vendor | Mode | Lane | Guarantee | Transport | Offline conformance | Verified live |
|---|---|---|---|---|---|---|---|
| `grok-bot` | grok | linked | `ui_bridge` | `best_effort` | CDP to the desktop app | yes | **Yes**, 2026-09-29, Grok Bot 0.61.0 (Electron 42.1): quit, relaunch with debug port, one message sent, app restored. Selector pack is v2. Approval-card and banner selectors are still inferred from the message catalog. |
| `claude-desktop` | claude | linked | `ui_bridge` | `best_effort` | CDP (default) or AX | yes | **No.** README status: offline-complete, not live-verified. The CDP route is probably vendor-blocked: the app checks a signed developer token before honoring a debug port, and Allternit does not try to bypass it. All selectors are unverified against the running app. |
| `claude-managed-agents` | claude | hosted | `official` | `exact` | Claude Managed Agents API | yes | **No.** Needs a real user-owned API key and environment id. There is no README in the adapter directory. `SUBS_GATEWAY_CLAUDE_MA_ENVIRONMENT_ID` must be set. |
| `chatgpt-dots` | openai | linked | `ui_bridge` | `best_effort` | Playwright browser (default) or native ChatGPT.app over AX | yes | **No.** README status: offline only. Composer, send, turn and login selectors are borrowed from `chatgpt-web` (live-checked 2026-09-27). The `/dots` routes and dot-specific selectors are inferred. Whether dot conversations show the usage-limit banner is unknown. Blocked on a consenting ChatGPT account. |
| `openclaw` | openclaw | hosted | `local` | `exact` | HTTP to the user's OpenClaw (default `http://127.0.0.1:18789`) | yes | **No.** No live OpenClaw was available. Endpoint paths and SSE shape follow the OpenAI-compatible convention and are verified only against `fixtures/fake-server.ts`. Conversation keying is not confirmed, so history is kept per context client-side. |
| Muse | meta | linked | `channel` | not declared in code | WhatsApp Business through `ChannelLaneTransport` | Rust test `muse_over_whatsapp_sends_and_returns_replies_as_agent_message_completed` (not run in this pass) | **No.** Blocked on a WhatsApp Business account and Meta app. Not an AAI adapter in the TS host. |
| `chatgpt-web` | openai | n/a | subsfab image lane | n/a | Playwright | conformance test `SG/chatgpt-web-conformance.test.ts` | Selectors live-checked 2026-09-27. It has no `aai.ts`, so it is not an AAI adapter. |
| `gemini-web` | google | n/a | subsfab web chat lane | n/a | Playwright (worker pool) | yes (`SG/web-chat-adapters.test.ts`) | **No.** Offline conformance passing; selectors inferred; NOT verified live. Written from public knowledge of gemini.google.com (rich-textarea/Quill composer, `/app/<id>` URLs); `readAccount` is DOM-only, usage null. Manual gate in the adapter README. |
| `gemini-subscription` | google | linked | `ui_bridge` | `best_effort` | browser session through the gateway's `gemini-web` lane | yes — offline conformance passing; selectors inferred; NOT verified live | **No.** Same shape as `kimi-subscription` (turns as the gateway's own chat tasks). Sign-in covers `gemini.google.com` + `accounts.google.com`. Blocked on an owner sign-in under Settings → Subscriptions. |
| `copilot-web` | microsoft | n/a | subsfab web chat lane | n/a | Playwright (worker pool) | yes (`SG/web-chat-adapters.test.ts`) | **No.** Offline conformance passing; selectors inferred; NOT verified live. Written from public knowledge of copilot.microsoft.com (cib-* elements, textarea composer); `/chats/<id>` thread URLs are unconfirmed (sidebar may own history); `readAccount` is DOM-only, usage null. Manual gate in the adapter README. |
| `copilot-subscription` | microsoft | linked | `ui_bridge` | `best_effort` | browser session through the gateway's `copilot-web` lane | yes — offline conformance passing; selectors inferred; NOT verified live | **No.** Same shape as `kimi-subscription` (turns as the gateway's own chat tasks). Sign-in covers `copilot.microsoft.com` + `login.live.com` + `login.microsoftonline.com`. Blocked on an owner sign-in under Settings → Subscriptions. |
| Loopback (Allternit bot) | allternit | native | n/a | n/a | HTTP to allternit-api | yes, passes every applicable area | Not a vendor. `SUBS_GATEWAY_AAI_LOOPBACK_BASE` and `_BOTS`, `_TOKEN` configure it. |

Manifest values are in each `manifest.ts`. `maxParallel` is 1 with `isolation: shared` for the three UI-bridge adapters, so placement serializes threads on them. `claude-managed-agents` and `openclaw` declare `isolated` contexts with `resume: true`.
