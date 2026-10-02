# gemini-subscription

Gemini through the user's own Google subscription (Settings → Subscriptions):
the shared subscription agent (`SubscriptionAgentProvider`) over the
`gemini-web` chat lane. Each AAI context is one gemini.google.com
conversation: its first message is a gateway `chat.create` task, later ones
`chat.continue` on the same thread (the gateway's thread mapping pins the
provider conversation and the owning Google account).

**Adapter id:** `gemini-subscription`. **Vendor id:** `google`. **AAI vendor
id:** `google`. **Agent id:** `gemini`. Lane `ui_bridge`, guarantee
`best_effort`, one context at a time per the shared manifest
(`maxParallel: 4`, `isolation: isolated`) — same shape as `kimi-subscription`.
`termsWarning` (in the manifest's auth descriptor): a UI-bridge lane drives
Gemini in a browser with the user's own subscription; it can break when the
site changes, and usage counts against the plan.

Turns run as the gateway's own chat tasks (`VendorContext.gatewayTasks`), so
login, usage limits, verification walls and pacing behave exactly like
Settings → Subscriptions chats. The account sign-in lives in `gemini-web`
(`manifest.yaml` origins + session cookies); this adapter only names the
provider (`google`).

**Offline conformance:** `POST /aai/conformance/gemini-subscription` (test
suite) and `POST /aai/conformance/gemini-subscription?offline=1` (running
gateway) pass against `fixtures/offline.ts` — a fake in-process task API, no
browser, no vendor.

**Live verification (human-driven, on a Sessions computer):**
1. Sign in under Settings → Subscriptions (Gemini / `google`) per the
   `gemini-web` README manual gate: the login browser opens at
   `https://gemini.google.com/app`, sign in by hand, the account reports
   `ready`.
2. `POST /aai/conformance/gemini-subscription?offline=1` — must pass.
3. Bind a bot (execution binding `adapter_id: gemini-subscription`,
   `account_binding_id` of the Google account), set it `READY`, send one turn:
   the reply lands in the thread as `agent.message.completed`.
4. Stop a turn mid-stream (`agent.context.cancel`) and confirm the Gemini
   window actually stops.
5. Confirm the account identity refreshes via `agent.list` (the discovery
   sync) and that a usage-limit response maps to `RATE_LIMITED` with
   `retryAfterMs`.
6. `POST /aai/conformance/gemini-subscription` (live, no `?offline=1`) —
   every critical check must pass against the signed-in UI; fix
   `gemini-web/selectors/v1.yaml` + fixtures for whatever drifted and bump
   `selectors_version`.

Never auto-approve a vendor prompt; a bot-check page returns `LANE_BLOCKED`
and latches (a person clears it in the Gemini window). Selector drift returns
`ADAPTER_DRIFT` and latches until the adapter is reset.
