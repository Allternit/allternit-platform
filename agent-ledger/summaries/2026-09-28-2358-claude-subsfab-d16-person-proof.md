# Agent Work Attestation — subsfab D16: only a person's session starts a subscription task

**Date:** 2026-09-28 23:58 CDT  
**Session ID:** subsfab-d16-fix  
**Branch:** session/subsfab-d16-fix (platform) · session/subsfab-human-proof (allternit-ai)  
**Agent:** claude (Opus 5.5)  
**Commit:** https://github.com/Gizziio/allternit-platform/pull/931 + https://github.com/Gizziio/allternit-ai/pull/230  
**Ledger entry:** [../LEDGER.md](../LEDGER.md)

## What was done

Fixes the step-4 review blocker that shipped to main with #924: any authenticated
caller, including gizzi's own runtime-device token, could decide a subscription
card and so mint `approval.confirm`. The unverified fix branch
(`wip/subsfab-d16-fix-agent`) refused known machine-token prefixes, but in
Desktop the person's UI and gizzi send the **same** paired device token
(`unified-main.ts` gives gizzi `session.accessToken` as `ALLTERNIT_API_TOKEN`
and injects the same token on the renderer's requests), so that fix would
have locked the person out of Desktop without closing the hole there.

One rule now holds on every surface: **only a person's Clerk session starts a
subscription task.**

- allternit-api `subscription_routes::person_acted` verifies a Clerk session
  token (header `X-Allternit-Human-Proof`, else the bearer) against Clerk's
  JWKS and requires `sub` == the acting user. Fails closed; only
  `ALLTERNIT_LOCAL_DEV_BYPASS` skips it.
- Required on every human act: deciding a subscription card
  (`POST /cowork/approvals`, mints `approval.confirm`), a send to a `subs-*`
  model through the chat bridge (mints `chat.send`),
  `POST /subscriptions/human-actions`, and `POST /subscriptions/disclosure/ack`.
  Refusals return `403 person_required`.
- Hops that carry the proof: allternit-api CORS, cloud-api CORS, cloud-api
  runtime relay allow-list (phone/web → Desktop), Electron's `allternit-api://`
  broker (already passes custom headers).
- allternit-ai: `personProofHeaders()` (fetch-interceptor token getter = Clerk
  in web, `getClerkToken` IPC in Desktop). Sent only on: subscription card
  decisions (permission store), composer sends marked `personSent` to `subs-*`
  models, disclosure acknowledgements. Bots, templates, scheduled jobs and the
  group-chat turn runner never set it, so their sends become a card.
- Step-5 (#926, merged meanwhile) majors closed by the same change: approving
  an MCP-prepared card now needs a person's session (the card is
  `actionType: subscription`), and the card now carries
  `subscription.{kind,provider,providerName,prompt}` so the person sees exactly
  what will be sent (the UI reads that field; before, the prompt sat only under
  `prepared`).
- Step-4 minors folded in: a stopped turn withdraws only its own card
  (`PermissionNext.withdraw`), declining one subscription card no longer
  rejects the session's other cards, the permission store restores a card the
  API refused instead of dropping it, the disclosure store ignores a load that
  finishes after the person cancelled.

## Verification

- allternit-api: `cargo test --lib -- subscription_routes cowork_routes v1_routes` 75 passed; wider run incl. `cors`/`auth` 147 passed. New tests sign real RS256 Clerk-style tokens (`auth::test_clerk_token`) and cover: person proof, web bearer, runtime-device / access / worker / service tokens, desktop header, forged, tampered, expired, another user's token.
- cloud-api: relay allow-list test extended (proof forwarded, cookie still dropped).
- gizzi-code: `bun test test/permission/next.test.ts test/provider/subscription-fabric.test.ts` 82 passed (new withdraw/decline test).
- allternit-ai: vitest 61 files / 402 tests passed across agents, chat, settings, subscriptions; stale-load test confirmed to fail without the fix. `tsc --noEmit`: no errors in changed files (the 18 remaining are office-suite/tldraw packages in the borrowed node_modules).
- Not yet live-verified in Desktop: needs a Desktop build from main with both PRs, then approve a card and send to ChatGPT from the composer.

## Limits (accepted, recorded on purpose)

1. **Same-user local agent.** gizzi runs as the person's macOS user. With real effort it could lift the Clerk session out of Electron's encrypted cookie store / the renderer and replay it. Clerk session tokens live ~60 s (+60 s verify leeway), which bounds the window. Closing this fully needs a native OS confirmation (Touch ID / system prompt) owned by Electron main.
2. **Replay inside the token lifetime.** A captured proof can be reused until it expires; it is not bound to a single request. A nonce-bound proof would need a server round trip per act.
3. **Clerk reachability.** The Desktop's local allternit-api verifies proofs against Clerk's JWKS (cached). If Clerk can't be reached and nothing is cached, human acts fail closed (403). Subscription use needs the network anyway.
4. **gizzi terminal (TUI/CLI).** The person typing and the agent are the same process, so a terminal send to a `subs-*` model always becomes a card to approve in web or Desktop.
5. **iOS.** Uses the agent-session protocol and has no subscription cards or `subs-*` sends yet. When it does, it sends the Clerk session as the proof; the cloud relay already forwards it.
6. **Paths that fall back to a card.** Code-mode sessions that talk to gizzi directly, CodeCanvas "retry last message", and bot-thread approvals of cards not in the permission store carry no proof, so they produce or keep a card rather than acting. Fail closed, by design.
7. **Dev bypass.** `ALLTERNIT_LOCAL_DEV_BYPASS=true` skips the check. The packaged Desktop does not set it (checked `backend-manager.ts` and the running process).

## Files changed

- `cmd/allternit-api/src/subscription_routes.rs` — `HUMAN_PROOF_HEADER`, `person_acted`; human-actions + disclosure ack require it; tests.
- `cmd/allternit-api/src/cowork_routes.rs` — subscription card decisions require a person; test.
- `cmd/allternit-api/src/subscription_mcp.rs` — MCP card shows the prompt; test.
- `cmd/allternit-api/src/v1_routes.rs` — `chat_send_human_action` mints only for a person; test.
- `cmd/allternit-api/src/auth.rs` — test-only Clerk token signer.
- `cmd/allternit-api/src/cors.rs`, `cmd/allternit-cloud-api/src/lib.rs` — allow the proof header.
- `cmd/allternit-cloud-api/src/routes/runtime_relay.rs` — relay forwards the proof header; test.
- `cmd/gizzi-code/src/runtime/tools/guard/permission/next.ts` — `withdraw`, no reject cascade for subscription cards.
- `cmd/gizzi-code/src/runtime/providers/fabric/human-gate.ts` — withdraw only its own card.
- `cmd/gizzi-code/test/permission/next.test.ts` — withdraw/decline test.
- allternit-ai: `src/lib/fetch-interceptor.ts`, `src/lib/agents/permission-store.ts`, `src/lib/agents/native-agent-api.ts`, `src/lib/agents/mode-session-store.ts`, composer views (`personSent`), `src/lib/subscriptions/disclosure-store.ts`, `src/lib/sessions-computer-api.ts`, tests.
