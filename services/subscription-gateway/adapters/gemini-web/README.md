# gemini-web

Gemini (Google) web UI adapter for the Subscription Gateway (`ui_bridge_web`).
Covers `chat.create` / `chat.continue` (pool `chat-msgs`) on
`https://gemini.google.com/app`. Built on the shared `WebChatAdapter` base
(fresh chat first, mapped-thread `chat.continue` with the divergence check
against `thread_mappings` fingerprints, fingerprint-based `reconcile` that
never resubmits).

**Vendor id:** `google`. **Sign-in domains:** `gemini.google.com` (app surface)
and `accounts.google.com` (Google account login); the account session cookies
(`SID` family) land on `.google.com` and both origins are in
`manifest.yaml` so the login watcher sees a sign-in on either domain.

**Selectors are v1-unverified.** `selectors/v1.yaml` was written from public
knowledge of the gemini.google.com UI (custom `<rich-textarea>` element
wrapping a Quill `.ql-editor`, material send/stop buttons with aria-labels,
`/app/<16-hex>` conversation URLs) and has NOT been validated against a
logged-in session. The fixtures are shaped to these selectors, so the suite
proves internal consistency, not live fidelity. Every strategy is marked
inferred in the file. `readAccount` is DOM-only (header avatar's accessible
name); it never reads a cookie or token, and usage stays `null` (hard limits
still surface through the banner checks before typing).

**Manual gate (human-driven, on a Sessions computer):**
1. Settings → Subscriptions: add **Gemini** (`google`). The login browser
   opens at `https://gemini.google.com/app`; sign in by hand (the Google
   account flow redirects through `accounts.google.com`). The watcher finishes
   the login by itself; the account must report `ready`.
2. `POST /aai/conformance/gemini-subscription?offline=1` — offline conformance
   passes without a browser (QUICKSTART step 2 shape).
3. Send one chat turn from a vendor-bound thread; the reply streams as deltas
   and completes. Confirm the signed-in identity shows for the account.
4. Confirm each `selectors/v1.yaml` key resolves against the live UI (the
   probe's per-key `lastMatchedStrategy` says which fallback matched), then
   fix the selectors + fixtures together and bump `selectors_version`.
5. Confirm the thread URL shape: open a past conversation and check the URL
   is `/app/<id>` as `THREAD_URL_PATTERN` expects.
6. `kill -9` the gateway mid-stream, restart, resend — reconcile adopts the
   thread or flags it ambiguous; verify in the Gemini UI nothing was
   double-submitted.

Challenges and login walls surface as `needs_user` and are never auto-retried
(Critical #5); a bot check latches the lane `LANE_BLOCKED` — a person clears
it in the Gemini window, never the adapter. Selector drift returns
`ADAPTER_DRIFT` and latches until the adapter resets.
