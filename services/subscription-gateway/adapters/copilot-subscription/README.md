# copilot-subscription

Copilot through the user's own Microsoft subscription (Settings →
Subscriptions): the shared subscription agent (`SubscriptionAgentProvider`)
over the `copilot-web` chat lane. Each AAI context is one
copilot.microsoft.com conversation: its first message is a gateway
`chat.create` task, later ones `chat.continue` on the same thread (the
gateway's thread mapping pins the provider conversation and the owning
Microsoft account).

**Adapter id:** `copilot-subscription`. **Vendor id:** `microsoft`. **AAI
vendor id:** `microsoft`. **Agent id:** `copilot`. Lane `ui_bridge`, guarantee
`best_effort`, one context at a time per the shared manifest
(`maxParallel: 4`, `isolation: isolated`) — same shape as `kimi-subscription`.
`termsWarning` (in the manifest's auth descriptor): a UI-bridge lane drives
Copilot in a browser with the user's own subscription; it can break when the
site changes, and usage counts against the plan.

Turns run as the gateway's own chat tasks (`VendorContext.gatewayTasks`), so
login, usage limits, verification walls and pacing behave exactly like
Settings → Subscriptions chats. The account sign-in lives in `copilot-web`
(`manifest.yaml` origins + session cookies); this adapter only names the
provider (`microsoft`).

**Offline conformance:** `POST /aai/conformance/copilot-subscription` (test
suite) and `POST /aai/conformance/copilot-subscription?offline=1` (running
gateway) pass against `fixtures/offline.ts` — a fake in-process task API, no
browser, no vendor.

**Live verification (human-driven, on a Sessions computer):**
1. Sign in under Settings → Subscriptions (Copilot / `microsoft`) per the
   `copilot-web` README manual gate: the login browser opens at
   `https://copilot.microsoft.com/`, sign in by hand (the Microsoft account
   flow redirects through `login.live.com` / `login.microsoftonline.com`),
   the account reports `ready`.
2. `POST /aai/conformance/copilot-subscription?offline=1` — must pass.
3. Bind a bot (execution binding `adapter_id: copilot-subscription`,
   `account_binding_id` of the Microsoft account), set it `READY`, send one
   turn: the reply lands in the thread as `agent.message.completed`.
4. Stop a turn mid-stream (`agent.context.cancel`) and confirm the Copilot
   window actually stops.
5. Confirm the account identity refreshes via `agent.list` (the discovery
   sync) and that a usage-limit response maps to `RATE_LIMITED` with
   `retryAfterMs`.
6. `POST /aai/conformance/copilot-subscription` (live, no `?offline=1`) —
   every critical check must pass against the signed-in UI; fix
   `copilot-web/selectors/v1.yaml` + fixtures for whatever drifted and bump
   `selectors_version`. Confirm how past conversations are addressed
   (sidebar vs `/chats/<id>` URL) and fix `THREAD_URL_PATTERN` accordingly.

Never auto-approve a vendor prompt; a bot-check page returns `LANE_BLOCKED`
and latches (a person clears it in the Copilot window). Selector drift
returns `ADAPTER_DRIFT` and latches until the adapter is reset.
