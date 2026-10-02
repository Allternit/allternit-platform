# copilot-web

Copilot (Microsoft) web UI adapter for the Subscription Gateway
(`ui_bridge_web`). Covers `chat.create` / `chat.continue` (pool `chat-msgs`)
on `https://copilot.microsoft.com/`. Built on the shared `WebChatAdapter` base
(fresh chat first, mapped-thread `chat.continue` with the divergence check
against `thread_mappings` fingerprints, fingerprint-based `reconcile` that
never resubmits).

**Vendor id:** `microsoft`. **Sign-in domains:** `copilot.microsoft.com` (app
surface), `login.live.com` and `login.microsoftonline.com` (Microsoft account
login); all three origins are in `manifest.yaml` so the login watcher sees a
sign-in on any of them (Microsoft account session cookies land on `.live.com`
/ `.microsoftonline.com`).

**Selectors are v1-unverified.** `selectors/v1.yaml` was written from public
knowledge of the copilot.microsoft.com UI (`cib-*` custom elements, a plain
textarea composer, the `#mectrl_headerPicture` account picture in the header)
and has NOT been validated against a logged-in session. The fixtures are
shaped to these selectors, so the suite proves internal consistency, not live
fidelity. Every strategy is marked inferred in the file. `readAccount` is
DOM-only (header account picture); it never reads a cookie or token, and
usage stays `null` (hard limits still surface through the banner checks
before typing).

**UNVERIFIED thread URLs:** consumer Copilot is believed to keep past
conversations behind the sidebar chat list rather than per-chat URLs.
`THREAD_URL_PATTERN` (`/chats/<id>`) is INFERRED. If live probing confirms no
per-chat URL, `chat.continue` must drive the sidebar's chat list instead, and
this pattern + `threadUrl` need rework.

**Manual gate (human-driven, on a Sessions computer):**
1. Settings → Subscriptions: add **Copilot** (`microsoft`). The login browser
   opens at `https://copilot.microsoft.com/`; sign in by hand (the Microsoft
   account flow redirects through `login.live.com` /
   `login.microsoftonline.com`). The watcher finishes the login by itself;
   the account must report `ready`.
2. `POST /aai/conformance/copilot-subscription?offline=1` — offline
   conformance passes without a browser (QUICKSTART step 2 shape).
3. Send one chat turn from a vendor-bound thread; the reply streams as deltas
   and completes. Confirm the signed-in identity shows for the account.
4. Confirm each `selectors/v1.yaml` key resolves against the live UI (the
   probe's per-key `lastMatchedStrategy` says which fallback matched), then
   fix the selectors + fixtures together and bump `selectors_version`.
5. Confirm how past conversations are addressed (sidebar vs `/chats/<id>`
   URL) and fix `THREAD_URL_PATTERN` accordingly.
6. `kill -9` the gateway mid-stream, restart, resend — reconcile adopts the
   thread or flags it ambiguous; verify in the Copilot UI nothing was
   double-submitted.

Challenges and login walls surface as `needs_user` and are never auto-retried
(Critical #5); a bot check latches the lane `LANE_BLOCKED` — a person clears
it in the Copilot window, never the adapter. Selector drift returns
`ADAPTER_DRIFT` and latches until the adapter resets.
