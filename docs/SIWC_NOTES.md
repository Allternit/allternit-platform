# Sign in with ChatGPT (SIWC) — Allternit Desktop pilot

Branch `siwc-desktop-pilot` (platform) + `siwc-desktop-pilot-ai` (allternit-ai worktree `../wt-siwc-ai`, commit `f2584d81`).
Nothing pushed, merged, or deployed.

Docs implemented: `developers.openai.com/siwc/token-sharing-open-source` (+ sign-in, profiles-and-sessions, models-and-inference, token-reference, errors-and-recovery, preview-limitations) and the cookbook article. Read as `.md` via curl.

## What it does
An eligible user clicks **Continue with ChatGPT** in Settings → Subscriptions (Desktop), authorizes in the system browser, and Allternit uses their ChatGPT plan for ChatGPT-model requests. No API key. **Default OFF** (`feature.siwc`), Desktop only.

## Files changed
Platform (`wt-siwc-desktop`):
- `surfaces/allternit-desktop/src/main/siwc.ts` — the flow: host id, dynamic registration, PKCE + loopback callback, ID-token validation (JWKS, iss, aud, exp, nonce), scope check, credential store, serialized refresh, revocation on sign out.
- `surfaces/allternit-desktop/src/main/siwc-broker.ts` — loopback token broker for gizzi-code (per-launch bearer secret; returns only the access token).
- `surfaces/allternit-desktop/src/main/siwc.test.ts`, `siwc-broker.test.ts`
- `surfaces/allternit-desktop/src/main/feature-flags.ts` — `feature.siwc`, default false.
- `surfaces/allternit-desktop/src/main/unified-main.ts` — manager, IPC (`siwc:status|signIn|cancel|signOut`), status push, broker env passed to the gizzi child.
- `surfaces/allternit-desktop/src/preload/index.ts`, `src/types/electron-api.d.ts` — `window.allternit.siwc` (status only).
- `cmd/gizzi-code/src/runtime/providers/siwc/{broker,discovery,language-model}.ts` — the provider.
- `cmd/gizzi-code/src/runtime/providers/{provider.ts,discovery/index.ts}` — wiring (see below).
- `cmd/gizzi-code/test/provider/siwc.test.ts`

allternit-ai (`wt-siwc-ai`): `src/views/settings/SiwcCard.tsx` (+ test), `SubscriptionsPanel.tsx`, `src/lib/globals.d.ts`.

## How it fits the provider seam
- Token custody: Desktop main only. Storage is `secure-store.ts` (Electron `safeStorage` → macOS Keychain when packaged; AES-GCM envelope in dev). Two secrets: `siwc-host-id` (`urn:uuid:…`, created before first sign-in) and `siwc-credentials` (record per issued client id + verified `sub`, holding the documented fields).
- gizzi-code (which makes the model calls) never sees the refresh token. It asks the broker (`ALLTERNIT_SIWC_BROKER_URL/TOKEN`, set on the spawned gizzi) for a fresh access token per request, then calls `POST https://api.openai.com/v1/responses` with `store:false, stream:true`, history in `input`, system text in `instructions`, none of the unsupported fields, success only on `response.completed`.
- The existing ChatGPT-subscription lane is `subs-chatgpt`. When SIWC is signed in with plan usage, `discoverSiwc()` runs **before** the fabric catalog and takes that lane id with the account's real models (`GET /v1/models`, `visibility=list`). A 30 s watcher swaps it in if the user signs in after gizzi started.
- Fallback: `SiwcLanguageModel` hands the turn to the existing web-chat adapter (`SubscriptionFabricLanguageModel`, class inferred from the slug) when SIWC cannot serve it *before any output*: not signed in, network failure, 401, 5xx. Usage limit (429 / `subscription_sharing_usage_limit_exceeded`), ineligible user, unsupported capability → surfaced to the user, never retried on the adapter. The lane is not deleted; with SIWC off or signed out, behaviour is unchanged.
- Web app: nothing runs. The env is only set by Desktop; the card renders only when `window.allternit.siwc` exists and the flag is on.

## Commands and results
- `surfaces/allternit-desktop`: `npx vitest run src/main/siwc src/main/mini-app-oauth-broker.test.ts` → siwc 24/24, broker 4/4 pass (mini-app OAuth 12/12 unchanged). `tsc -p src/main/tsconfig.json --noEmit` → no errors in any touched file.
- `cmd/gizzi-code`: `bun test test/provider/siwc.test.ts` → 12/12. `bun test test/provider/subscription-fabric.test.ts` → 13/13 (two D16 card tests timed out once while a tsc ran in parallel; the same file passes 13/13 on a clean HEAD checkout and on two reruns here). `tsc --noEmit` → no errors in the touched files.
- `wt-siwc-ai`: `npx vitest run src/views/settings` → 37/37 (SiwcCard 7). Scoped `tsc` on `SiwcCard.tsx`, its test and `globals.d.ts` → clean.
- Tests cover: flag gate (no network/secret/browser when off; token stops the moment the flag turns off), authorize URL parameters, ID-token checks (signature, iss, aud, exp, nonce), declined consent, wrong-state callback ignored, missing issued client id, plan-usage scope missing, refresh (issued client id, `resource`, scope omitted, rotation persisted, concurrent refreshes = one request, `invalid_grant` clears tokens but keeps mapping, transient failure keeps credentials), sign out (revocation body, tokens cleared, host id + client mapping kept, unconfirmed revocation reported), returning sign-in reuses client id and host id, different-identity rejection, consent re-prompt only on explicit enable, broker auth, fallback rules, request shape.
- Not run: no live sign-in against auth.openai.com (needs a real ChatGPT account and approval, below). No builds/dev servers.

## Cuts (smallest complete version)
- No live end-to-end run against OpenAI; verified against a fake OpenAI only.
- One active account in the UI. Storage is keyed per issued client id (as documented), but there is no account picker / switching UI.
- Host id is `urn:uuid:` (supported); the recommended JWK-thumbprint form is not implemented.
- Text and image inputs only. No tools: SIWC models are registered with `toolcall:false` like the other subscription lanes (the route supports function tools; not wired). No reasoning-summary stream. Context/output limits are conservative constants (128k/32k) because the catalog call does not report them here.
- `earliest_refresh_at` is ignored: refresh happens 60 s before expiry.
- 401 from the Responses API falls back to the adapter but does not proactively invalidate the saved session; the next refresh confirms disconnection.
- Only the Desktop-spawned gizzi gets the broker env. The always-on launchd gizzi daemon does not (a per-launch secret must not go in a plist), so SIWC is unavailable when Desktop attaches to that daemon.
- OpenAI-branded button assets not used; plain "Continue with ChatGPT" text per the doc's label. Needs a UI/UX guidelines pass before release.

## What Eoj must do
1. **OpenAI approval.** The docs cover open-source / locally run apps. Allternit Desktop is a paid, hosted-account product → submit OpenAI's interest form (`openai.com/form/sign-in-with-chatgpt-interest/`) before turning the flag on for anyone but yourself.
2. **Client id.** None to register: this flow uses dynamic registration (`client_id=dynamic_agent_client`, no secret, no partner key); the issued `oaiapp_…` id is per user and saved on their machine. The user names/approves the agent in the browser (`agent_name_hint=Allternit`). Confirm that name is what you want shown on the consent screen.
3. **UI/UX guidelines and branding** (`developers.openai.com/siwc/ui-ux-guidelines`): confirm the button/entry meets their approved-branding rules.
4. **Live test**: set `ALLTERNIT_FLAG_FEATURE_SIWC=1` (or `"feature.siwc": true` in `~/.allternit/flags.json`), restart Desktop, sign in with a real Plus/Pro account, send a ChatGPT-model message, and check ChatGPT Settings → Usage shows the app.
5. Decide whether SIWC should ever ship in the packaged release or stay a developer flag until approval.

## Does Desktop's license qualify as "open-source"?
Probably not on the Desktop app itself, so don't assume it does:
- Repo `LICENSE` and `cmd/gizzi-code` are Apache-2.0 (open source). `NOTICE` says the ai.allternit.com UI is proprietary and lives in the private `Gizziio/allternit-ai` repo — and the Settings card is in that repo.
- `surfaces/allternit-desktop/package.json` says `"license": "UNLICENSED"`; its installer license (`build/LICENSE.txt`) reads as a permissive MIT-style text; the root `package.json` says MIT. These disagree with each other.
- Even if the Desktop source were clearly OSI-licensed, it is sold as a paid product with account/billing, which is the "paid or remotely hosted app" the docs send to the interest form. Treat approval (item 1) as required. Reconciling the Desktop license files is a separate cleanup I did not touch.

status: done
