# Zoom: cloud side for the Zoom adapter (step 2.3)

This is what cloud-api must add so the Zoom adapter (`services/meeting-bot/src/adapters/zoom/`) can join a real call. The bot side is built: an adapter plus a Meeting SDK for Linux sidecar. This page is the backend lane's spec. **The routes in sections 3 and 4 are not in CONTRACT.md**, so they need a contract update before anyone builds them (see section 9).

Sources were read on 2026-11-13:
- OBF auth: https://developers.zoom.us/docs/meeting-sdk/auth/obf/
- OBF FAQ: https://developers.zoom.us/docs/meeting-sdk/obf-faq/
- SDK JWT: https://developers.zoom.us/docs/meeting-sdk/auth/
- OAuth: https://developers.zoom.us/docs/integrations/oauth/
- Linux SDK overview: https://developers.zoom.us/docs/meeting-sdk/linux/
- Linux SDK download: https://developers.zoom.us/docs/meeting-sdk/linux/get-started/download/
- Raw data: https://developers.zoom.us/docs/meeting-sdk/linux/custom-ui/advanced-features/raw-data/
- Review: https://developers.zoom.us/docs/distribute/app-review-process/ and https://developers.zoom.us/docs/distribute/app-review-guidelines/

## 1. How a Zoom join works

1. The user connects Zoom once (OAuth, section 3). cloud-api stores their refresh token, encrypted.
2. When a session launches for a Zoom meeting, cloud-api does three things. It refreshes the access token. It fetches an **OBF token**: `GET https://api.zoom.us/v2/users/me/token?type=onbehalf` with `Authorization: Bearer <access token>`, which returns `{"token": "..."}` and lives 2 hours by default. It signs an **SDK JWT** with the app's client id and secret.
3. The pod gets `identity = { kind: "zoom_obf", obfToken, sdkKey: <client id>, sdkSecretRef: "jwt:<signed SDK JWT>" }` (adapter.ts `BotIdentity`). The `jwt:` form keeps the client secret in cloud-api. The adapter also accepts `env:<NAME>` (it then signs in the pod), but use that only for own-account dev tests.
4. The sidecar calls `SDKAuth` with the JWT, then `Join` with `onBehalfToken` set to the OBF token.

Zoom's rules that shape the product:
- "OBF apps can only join a meeting once their verified associated owner is in a meeting or webinar." The bot must leave when the owner leaves. In the FAQ's words, "the session ends when that user leaves the meeting". So **the user who connected Zoom must be in the call for Gizzi to be there.** While Zoom reports them absent, the adapter retries the OBF join every 20 s within `admitTimeoutMs` and reports `waiting_admit` (page label "Waiting for you to join Zoom"). If they never join, it ends with `not_admitted`.
- Zoom FAQ: "Enforcement began March 2, 2026." Anonymous Meeting SDK joins are refused. There is no web-client fallback.
- Zoom (OBF FAQ): "All apps must go through app review in order to access meetings or webinars hosted outside the developer account that created the app." Until the app is approved, the adapter works only on meetings hosted by our own Zoom account (allternitpbc).
- Minimum SDK version is 5.17.5. Use 6.6.10 or later, which has clearer OBF errors.

## 2. The Marketplace app (already made)

| Field | Value |
|---|---|
| Type | General app, user-managed, Meeting SDK on, "meeting bot / AI notetaker", OBF required, ZAK off |
| App id | `Xie3OlJ3RBalFDC8IZCIfA` (display name still "General app 780"; rename it in the listing) |
| Dev client id | `CiBOKbUnToOnjRJgZmvHQQ` |
| Dev client secret | macOS Keychain, service `allternit-zoom-app-dev`. Never print it. Put it in cloud-api secrets as `ZOOM_CLIENT_SECRET`. |
| Redirect + allow list | `https://api.allternit.com/api/v1/zoom/oauth/callback` (use exactly this path) |
| Scopes | `user:read:token` (OBF token), `meeting:read:local_recording_token` (local-recording join token) |
| Production credentials | Not yet. Zoom: submit first-time review with the production client id. |

cloud-api secrets: `ZOOM_CLIENT_ID`, `ZOOM_CLIENT_SECRET`, `ZOOM_WEBHOOK_SECRET_TOKEN` (from the app's Feature page, for the deauthorization webhook). Dev and prod are separate pairs.

We need no extra scope. The UI can say "Zoom connected" without the account email. Add `user:read:user` only if the UI must show the email. Zoom review checks for minimum scopes.

## 3. OAuth connect (proposed routes)

Same pattern as the calendar OAuth routes in CONTRACT.md section 4.

| Route | Auth | Does |
|---|---|---|
| `GET /api/v1/zoom/oauth/start?return_to=` | User | `200 { url }`. The URL is `https://zoom.us/oauth/authorize?response_type=code&client_id=<ZOOM_CLIENT_ID>&redirect_uri=https%3A%2F%2Fapi.allternit.com%2Fapi%2Fv1%2Fzoom%2Foauth%2Fcallback&state=<signed>`. `state` is signed server-side as `{user_id, return_to, nonce, exp 10 min}`, the same as the calendar routes. PKCE (`code_challenge`, S256) is optional for a confidential client. Add it if the state already carries a verifier. |
| `GET /api/v1/zoom/oauth/callback?code=&state=` | none (state-verified) | Verify `state`. Exchange the code with `POST https://zoom.us/oauth/token`, header `Authorization: Basic base64(client_id:client_secret)`, form `grant_type=authorization_code&code=<code>&redirect_uri=<exact redirect>`. Store the tokens (section 5). As a check, call the OBF endpoint once and discard the token. Then `302` to `return_to` (default `https://ai.allternit.com/settings/meetings`) with `?zoom=connected` or `?zoom=error`. |
| `GET /api/v1/zoom/connection` | User | `200 { connected: bool, status, connected_at }`. Never returns tokens. |
| `DELETE /api/v1/zoom/connection` | User | Best effort: `POST https://zoom.us/oauth/revoke` with Basic auth and `token=<access token>`, which returns `{"status":"success"}`. Then delete the row. `204`. |
| `POST /api/v1/zoom/webhook` | Zoom signature | See section 6. |

Token facts (Zoom OAuth page): access tokens last one hour. Refresh tokens last 90 days, and each refresh returns a new refresh token: "always use the latest refresh token for the next refresh request". Refresh with `POST https://zoom.us/oauth/token`, Basic auth, `grant_type=refresh_token&refresh_token=<latest>`. If the refresh token has expired, the user has to connect again.

## 4. Pod token route (proposed)

Use this when launch-time tokens are too old by join time. The OBF token lives 2 h, which is enough for a launch. The local-recording join token lives 120 s, which is not.

`POST /api/v1/meeting-sessions/:sid/zoom-tokens` (Pod bearer, the same pod token as ingest) returns `200 { obf_token, sdk_jwt, app_privilege_token | null }`.
- It mints a fresh OBF token for the session owner (refreshing the access token if needed).
- `sdk_jwt` is signed now (section 7).
- `app_privilege_token`: `GET https://api.zoom.us/v2/meetings/{meetingNumber}/jointoken/local_recording` with the user's access token returns `{ token, expire_in: 120 }`. Forum reports say it works only when the user hosts the meeting. On any 4xx, return `null` and the sidecar asks the host instead (sidecar README, "Raw audio permission"). Personal-room links (`zoom:my:<name>`) have no number: return `null`.
- Errors: `409 session_not_active`, `409 zoom_not_connected` (no row, or `needs_reauth`; same code as on `POST /meetings/:id/sessions`, CONTRACT.md).

The adapter already takes this through `ZoomAdapterOptions.freshTokens`. The runner wires that option to this route.

## 5. Storage (proposed migration, next free number after 130)

`zoom_connections`, one row per user:

| column | type | notes |
|---|---|---|
| id | uuid PK | |
| user_id | text | UNIQUE |
| encrypted_refresh_token | text | `state.credential_cipher`, same as `calendar_connections` |
| encrypted_access_token | text null | cached |
| access_expires_at | timestamptz null | refresh when less than 5 min remain |
| scopes | text[] | from the token response `scope` |
| status | text | CHECK `active|needs_reauth|revoked`, default `active` |
| created_at, updated_at | timestamptz | |

Refresh-token rotation means two pods refreshing at once can lose a token. Refresh inside a transaction with `SELECT ... FOR UPDATE` on the row, then write the new refresh token before the transaction commits. On `invalid_grant`, set `needs_reauth`. The Meetings page then shows "Reconnect Zoom", and launches for that user end with `join_failed` ("reconnect Zoom").

## 6. Deauthorization webhook (required for Marketplace)

Zoom: when a user removes the app, it POSTs `app_deauthorized` to the deauthorization endpoint set on the App Listing page. "Apps must delete all associated user data." Set that endpoint to `https://api.allternit.com/api/v1/zoom/webhook`.
- **Verify every request.** Header `x-zm-signature` must equal `v0=` + hex HMAC-SHA256 over `v0:<x-zm-request-timestamp>:<raw body>`, keyed with `ZOOM_WEBHOOK_SECRET_TOKEN`. Reject a timestamp older than 5 minutes.
- **URL validation.** Event `endpoint.url_validation` arrives with `payload.plainToken`. Reply `200 { plainToken, encryptedToken: hex HMAC-SHA256(secret token, plainToken) }`.
- **`app_deauthorized`.** Find the row by the Zoom user id in the payload, delete it, and end any active Zoom session for that user with `left_by_user`. Matching by Zoom user id means we must store it at connect time: put a `zoom_user_id` column on the row, filled from the token response or the OBF check. If neither gives it, this is the one reason to add `user:read:user` and call `GET /v2/users/me`.
- These verification details are from Zoom's webhook docs as generally documented and were not re-read today. Check the Feature page's "verify" sample when you build it.

## 7. SDK JWT

Zoom: header `{"alg":"HS256","typ":"JWT"}`. Payload `appKey` (client id), `iat`, `exp`, `tokenExp`; `mn` and `role` are optional for native SDKs. `exp` and `tokenExp` must be at least 1800 s after `iat`, and at most 48 h. Sign with HMAC-SHA256 using the client secret, server-side. The reference code is `services/meeting-bot/src/adapters/zoom/auth.ts` `signSdkJwt`: it back-dates `iat` by 30 s for clock skew and defaults to a 2 h lifetime. Mint one per session and never cache it across users.

## 8. Rate limits and volume

Zoom labels the token endpoints with rate-limit classes (the local-recording token endpoint is "LIGHT"). The per-second numbers depend on the account plan and are listed at https://developers.zoom.us/docs/api/rate-limits/ (not re-read today). Our volume is small: one OBF fetch per session, and one bot per `url_key`. Handle `429` with `Retry-After` and one retry. Never fetch tokens in a loop for a waiting pod.

## 9. Contract decisions (2026-11-13, now in CONTRACT.md "Change 2026-11-13: Zoom cloud side")

1. **Accepted:** the routes above (`/zoom/oauth/*`, `/zoom/connection`, `/zoom/webhook`, `/meeting-sessions/:sid/zoom-tokens`), the `zoom_connections` table, and the `zoom_not_connected` (409) code.
2. **Accepted:** `BotIdentity.zoom_obf.sdkSecretRef` is `jwt:<SDK JWT signed by cloud-api>` in production and `env:<NAME>` for local dev only.
3. **Accepted and built:** the owner-present retry inside the Zoom adapter. It retries every 20 s within `admitTimeoutMs` and reports `waiting_admit`. The runner still makes one `join()` call. The adapter.ts interface comment now allows retries inside an adapter.
4. **Accepted:** the local-recording join token arrives through `freshTokens`, from the zoom-tokens route. It is refetched on retries more than 100 s after the last fetch.

## 10. Marketplace review (what must be true before submit)

From the review docs, our setup (`meetings-gates/zoom-marketplace-app.md`) and Zoom staff:
- **Eligibility.** The Linux SDK overview says: "The Meeting SDK is designed for human use cases and does not support bots or AI notetakers. To build an AI notetaker application or access realtime media, use Zoom RTMS." The OBF FAQ, however, says older note-taking apps can either adopt OBF or move to RTMS. On 2026-07-23 a Zoom staff member answered a question about a Linux SDK + OBF facilitator bot: "this use case is still valid, you will need to go thru marketplace submission for this scenario" (https://devforum.zoom.us/t/marketplace-review-eligibility-for-commercial-ai-facilitator-bot-using-linux-meeting-sdk-obf/144959). **Risk:** review could still steer us to RTMS. RTMS needs the host's admin to turn it on, so it does not cover meetings hosted by other companies.
- **Notetaker disclosure.** The listing must say plainly that Gizzi joins as a visible participant named "Gizzi (Allternit)" and only while you are in the meeting. In the call it posts a notice that it is transcribing, asks the host for recording permission, keeps text only (no audio), and leaves when you leave or when removed. The in-call notice text is `noticeText` (consent-copy.md). The sidecar also triggers Zoom's own recording notice when the host grants local recording.
- A deauthorization webhook that deletes tokens (section 6).
- Privacy policy and terms covering Zoom data. The Zoom paragraph is in `privacy-terms-additions.md`.
- Live support and docs URLs. The docs page is `guides/meetings.mdx` with a Zoom section.
- A test plan for reviewers: they connect Zoom, start a meeting they host, ask Gizzi to join, see the notice, chat with Gizzi, grant recording, see the transcript, remove the app, and see the tokens deleted. Also a demo video of the same.
- Technical design and security answers: TLS 1.2+, tokens encrypted at rest (`credential_cipher`), no client secret in the pod, transcript retention.
- Production client id and secret, with the redirect URL and webhook set on the production side.

## 11. What blocks a real Zoom test today

1. **The Linux SDK tarball.** It downloads only while signed in to the Marketplace (sidecar README, "Getting the SDK"). After that: `bash build.sh <tarball>` on a Linux x86_64 or arm64 host with PulseAudio, then fix whatever the real headers reject (the sidecar README lists what to expect).
2. **A user who has authorised the app, and an OBF token.** The cloud routes in section 3 don't exist yet. For a first own-account test before they exist: sign in to Zoom as allternitpbc and authorise the dev app once through the authorize URL (redirect to the registered callback, read `code` from the address bar). Exchange it with curl using the Keychain secret, never echoing it. Fetch the OBF token, then run the pod with `sdkSecretRef: "env:ZOOM_CLIENT_SECRET"`.
3. **A meeting hosted by allternitpbc, with allternitpbc in it.** Before review approval, OBF works only on our own account's meetings, and the owner must be present. A free (Basic) account caps meetings at 40 minutes, which is enough for the test.
4. Pass criteria (plan step 2.3): joined via OBF, chat round trip, audio captured to the STT path.
