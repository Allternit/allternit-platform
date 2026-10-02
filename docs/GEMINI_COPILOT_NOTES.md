# GEMINI_COPILOT_NOTES — Gateway: Gemini and Copilot as carried vendors (platform side)

Session branch: `gateway/gemini-copilot-adapters` (worktree `allternit-wt-gemini-copilot`).
Everything below was built and proven offline against fixtures. No vendor was contacted:
no browser opened to a vendor, no sign-in, no message sent, no logo or proprietary asset
copied (no `look-profile.json` shipped — Kimi's adapters ship none either).

## Adapter ids and vendor ids

| Vendor | Web adapter id | AAI adapter id | Vendor id (AAI + Settings provider key) | Agent id | Login URL |
|---|---|---|---|---|---|
| Gemini (Google) | `gemini-web` | `gemini-subscription` | `google` | `gemini` | `https://gemini.google.com/app` |
| Copilot (Microsoft) | `copilot-web` | `copilot-subscription` | `microsoft` | `copilot` | `https://copilot.microsoft.com/` |

## Per-file changes

New files:

- `services/subscription-gateway/adapters/gemini-web/manifest.yaml` — §S2 manifest,
  `provider: google`, origins `gemini.google.com` + `accounts.google.com` (the
  Google-account login domain, same pattern as chatgpt-web's `auth.openai.com`),
  `session_cookies: [SID, __Secure-1PSID]` (INFERRED, commented), plans
  free/ai_pro/ai_ultra, `chat.create`/`chat.continue` on pool `chat-msgs`, Kimi pacing.
- `services/subscription-gateway/adapters/gemini-web/selectors/v1.yaml` — all 12 named
  keys (composer, send_button, stop_button, response, user_turn, streaming,
  logged_in_probe, logged_out_probe, model_picker, banner, challenge,
  account_identity), every strategy marked inferred with a comment saying what it
  was inferred from; role/aria/element-name hooks before class names.
- `services/subscription-gateway/adapters/gemini-web/adapter.ts` — `GeminiWebAdapter`
  over the shared `WebChatAdapter`; `THREAD_URL_PATTERN` `/app/<16-hex>` (inferred);
  Gemini banner patterns; DOM-only `readAccount` (header avatar accessible name →
  identity, usage null — Gemini has no known non-spending usage RPC).
- `services/subscription-gateway/adapters/gemini-web/fixtures/{idle,streaming,complete,logged-out,challenge,limit-banner}.html` —
  the same six canonical pages as Kimi, shaped to the selectors.
- `services/subscription-gateway/adapters/gemini-web/README.md` — status + exact
  live-verification steps.
- `services/subscription-gateway/adapters/copilot-web/{manifest.yaml,selectors/v1.yaml,adapter.ts,fixtures/*.html,README.md}` —
  the same shape for Microsoft: origins `copilot.microsoft.com` + `login.live.com` +
  `login.microsoftonline.com`, `session_cookies: [MSPAuth, MSPProf, __Host-MSAAUTH]`
  (INFERRED); `THREAD_URL_PATTERN` `/chats/<id>` flagged UNVERIFIED (consumer Copilot
  may keep history sidebar-only — see "needs live confirmation").
- `services/subscription-gateway/adapters/gemini-subscription/{index.ts,aai.ts,fixtures/offline.ts,README.md}` —
  `SubscriptionAgentProvider` spec over the `gemini-web` lane; offline fixtures are a
  fake in-process task API (mirrors `kimi-subscription/fixtures/offline.ts`).
- `services/subscription-gateway/adapters/copilot-subscription/{index.ts,aai.ts,fixtures/offline.ts,README.md}` —
  same for `copilot-web`.

Modified files:

- `services/subscription-gateway/src/http/routes_accounts.ts` — `KNOWN_PROVIDERS`
  gains `google: "Gemini"`, `microsoft: "Copilot"` (Settings → Subscriptions hub).
- `cmd/allternit-api/src/agent_gateway_routes.rs` — `adapter_for_auth` maps
  `("google", "browser_session") → gemini-subscription` and
  `("microsoft", "browser_session") → copilot-subscription`; the unit test gains the
  two mappings plus negative cases (`api_key`/`desktop_session` → None).
- `services/subscription-gateway/test/web-chat-adapters.test.ts` — `CASES` entries for
  both web adapters (conformance, thread-URL good/bad, fresh-chat e2e, challenge,
  limit-banner, logged-out, divergence adopt/fail/fork, reconcile); registry
  assertions; `createAdapter()` assertions; two new `readAccount` describes.
- `services/subscription-gateway/test/accounts-hub.test.ts` — providers list now
  asserts `google`/`microsoft` names with `supported: false` when no adapter is
  loaded (the hub is adapter-driven).
- `docs/gateway/ADAPTERS.md` — four rows in the shipped-adapters table, each marked
  "offline conformance passing; selectors inferred; NOT verified live".

## Every place Kimi was wired, and what was mirrored there

1. `adapters/kimi-web/` → built `adapters/gemini-web/` + `adapters/copilot-web/`
   (manifest.yaml, selectors/v1.yaml, adapter.ts, six fixtures, README).
2. `adapters/kimi-subscription/` → built `adapters/gemini-subscription/` +
   `adapters/copilot-subscription/` (index.ts, aai.ts, fixtures/offline.ts, README).
   The AAI host discovers `adapters/*/aai.ts` at boot (`registerVendorAdapters`), so
   no host change was needed — same as Kimi needed none.
3. `cmd/allternit-api/src/agent_gateway_routes.rs::adapter_for_auth`
   `("kimi","browser_session") → "kimi-subscription"` → added the google/microsoft
   rows and test assertions.
4. `services/subscription-gateway/src/http/routes_accounts.ts::KNOWN_PROVIDERS`
   (`kimi: "Kimi"`) → added `google`/`microsoft` names.
5. Web-adapter manifest `origins` (Kimi lists www.kimi.com beside kimi.ai so the
   login watcher sees cross-domain sign-in cookies) → gemini-web lists
   `accounts.google.com`; copilot-web lists `login.live.com` +
   `login.microsoftonline.com`. The existing multi-domain mechanism
   (`sessionCookies` host-suffix matching in routes_accounts.ts) handles both with
   no code change.
6. `services/subscription-gateway/test/web-chat-adapters.test.ts` (kimi-web CASE +
   kimi readAccount describe) → mirrored both.
7. `services/subscription-gateway/test/aai-conformance-route.test.ts` and the
   offline-fixtures guard — directory-driven; the two new adapters are picked up
   automatically (this is the "adapter guard/conformance" extension: both now pass
   `POST /aai/conformance/<id>` and `?offline=1` paths for every area their
   manifest supports — identity, context, parallelism, failure, idempotency,
   cancellation, events; areas the manifest doesn't declare are skipped).
8. `services/subscription-gateway/test/accounts-hub.test.ts` (kimi in the provider
   list test) → extended with both new provider names.

Not mirrored, deliberately: `adapterForVendor` in `routes_aai.ts` (Kimi is not in
that switch either — subscription bindings carry their `adapterId` from
`adapter_for_auth`), the CLI-provider catalog in `provider_routes.rs`
(`kimi-cli` is a different surface, not the gateway), and `main.ts` `gatewayTasks`
(provider-agnostic, keyed by the spec's `provider`). Usage-reading hook
(`readAccount`), identity reader (`discover_agents`/`refresh_agent_identity`), plan
table (`manifest.plans`), pacing (`manifest.pacing`) are all manifest/spec-driven —
no vendor-name branches, so nothing to add.

## Test / typecheck / cargo summary

- `pnpm install --frozen-lockfile` — OK (7m13s, pnpm 10.28.0).
- `pnpm -C services/subscription-gateway typecheck:noemit` — clean.
- `npx vitest run test/web-chat-adapters.test.ts` — 43/43 passed (includes both new
  adapters: conformance over the 6 fixtures, fresh-chat e2e, divergence, reconcile,
  thread URLs, readAccount).
- `pnpm -C services/subscription-gateway test` — 548/549 passed; the single failure
  was `chatgpt-web-conformance.test.ts > image.generate partial tiles`, a 30s
  timeout while the whole suite ran concurrently with a 22-minute `cargo check`.
  Re-run of that file alone: 31/31 passed. The failure is load-related and
  pre-existing behavior in a file this change does not touch; no file under any
  other vendor's `adapters/` directory changed.
- `bash scripts/check-fabric-sources-clean.sh` — OK.
- `CARGO_TARGET_DIR=$HOME/Desktop/allternit-workspace/.shared-target cargo check -p
  allternit-api` — finished clean (0 errors; 93 pre-existing warnings).
- Smoke boot on a fresh database: see the last section for the exact command and
  result.

## Selectors that need live confirmation

Every strategy in both `selectors/v1.yaml` files is inferred; these are the ones
most likely to be wrong or to rot first:

gemini-web:
- composer `rich-textarea .ql-editor[contenteditable=true]` (Quill inside a custom
  element, possibly shadow-DOM — if shadow-closed, needs a piercing fallback).
- send/stop accessible names ("Send message" / "Stop response").
- response `.model-response-text` / `.response-container .markdown`; user turn
  `.user-prompt`.
- logged-in probe: header avatar button `button[aria-label*='Google Account']`.
- thread URL `/app/<16-hex>` and the id charset.
- session cookie names `SID` / `__Secure-1PSID` (Google rotates these).
- challenge shape (Google's own bot checks vs Cloudflare).
- whether a non-spending usage counter exists anywhere (readAccount currently
  returns usage null by design).

copilot-web:
- thread URLs: whether `/chats/<id>` exists at all — consumer Copilot is believed
  to keep history in the sidebar; if so, chat.continue must drive the sidebar and
  `THREAD_URL_PATTERN` needs rework.
- composer `textarea[data-testid='composer-input']` / `cib-text-input textarea`.
- send/stop accessible names; turn containers `.turn-bot` / cib-message-group
  classes; `.ac-textBlock` answer text.
- logged-in probe / account identity `img#mectrl_headerPicture` (the Microsoft
  consumer header id).
- session cookie names `MSPAuth` / `MSPProf` / `__Host-MSAAUTH` and which domain
  they land on.
- model/mode picker shape ("Auto" / "Copilot" / "Think").

Both: the usage-limit banner copy (banner regexes were written from each vendor's
public limit wording) and the logged-out header shape (Sign in button label).

## Not done, and why

- No live verification (no sign-in, no vendor traffic) — by design per the task's
  hard rules; the exact human steps are in each adapter README and mirror
  OPERATIONS.md's live-verification checklist.
- No `look-profile.json` and no web look pack — Kimi's adapters ship neither, and
  the task's monogram rule forbids copying logos; the packs are a web-repo
  (`allternit-ai`) concern per LOOK_PACKS.md.
- `adapterForVendor` (`routes_aai.ts`) not extended — Kimi isn't in it; bindings
  carry the adapter id chosen by `adapter_for_auth`.
- The one flaky chatgpt-web image test noted above — unrelated to this change;
  passes in isolation.
- Rust unit test `discovery_uses_the_adapter_the_account_was_connected_with` was
  compile-checked via `cargo check` but not executed (the task's Verify section
  calls for `cargo check` + a smoke boot; building the full test binary was out of
  proportion). The assertions are the same shape as the existing kimi lines.

## Smoke boot (fresh database, route-overlap proof)

Command (from the repo root, shared target dir per the task rules):

```
SCRATCH=$(mktemp -d /tmp/gw-smoke.XXXX)
ALLTERNIT_DATA_DIR=$SCRATCH ALLTERNIT_API_PORT=39113 \
  $HOME/Desktop/allternit-workspace/.shared-target/debug/allternit-api
```

Result: **PASS.** Booted on a fresh `ALLTERNIT_DATA_DIR` (scratch
`/tmp/gw-smoke.RIJ3`, deleted after): refinery applied the full migration set
(211 migration log lines, no panic — the old V33 duplicate noted in
`docs/learnings` is fixed on this branch), the axum router built and listened
on 0.0.0.0:39113 (an overlapping route definition panics before listen, so
listening is the no-overlap proof). Probes: `GET /health` → 200;
`GET /api/v1/gateway/provider-accounts` → 401 (mounted, auth-gated, not 404);
`GET /api/v1/gateway/bots/bot-1/execution-binding` → 401;
`GET /api/v1/gateway/vendor-packs/google/parity` → 401 — the new vendor id
routes through the existing route table with no new routes added. Process
stopped and scratch removed; the installed Desktop app's gateway (port 8013)
was not touched.

status: done
