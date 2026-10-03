# Voice runtime notes (Track E)

**status: partial.** All four routes, tests, smoke boot and docs are done. The routes answer 503 to signed requests until allternit-api can read its own device token (below).

## Branch and commits
Pushed `ao/voice-runtime` (rebased on origin/main 3e93cfcb8, no PR opened):
- `8a3098bd3` voice: runtime routes for relayed phone calls
- `d5ebca8f7` docs: voice call runtime routes and signature scheme
- the notes/task/sentinel commit (separate, last)

## Migration
`V215__voice_calls.sql` (origin/main tops out at V214; rechecked after the final rebase, still free). Applied on a fresh DB in the smoke boot.

## Files touched
- `cmd/allternit-api/migrations/V215__voice_calls.sql` (new)
- `cmd/allternit-api/src/voice_calls.rs` (new: `RelayedVoiceAuth`, `VoiceRelaySecret`, 4 routes, tests)
- `cmd/allternit-api/src/lib.rs` (`pub mod voice_calls;`)
- `cmd/allternit-api/src/main.rs` (one `.merge(voice_calls_router())` in the public router)
- `cmd/allternit-api/docs/VOICE_CALLS.md` (new)
- `surfaces/docs/guides/voice-call-runtime-routes.mdx` (new), `surfaces/docs/docs.json` (nav)

## Tests
- `cargo test -p allternit-api --lib voice_calls`: `10 passed; 0 failed` (verifier: good/bad/stale/future/wrong owner/body+method+path tamper/unconfigured; routes: auth, idempotent create, events order/dup/interim/404/ended, turn stream, turn error, new-turn abort, DELETE abort).
- Full `cargo test -p allternit-api --lib` (before the final rebase): `1969 passed; 1 failed`. The failure was `webhook_subscription_routes::tests::signed_delivery_to_matching_subscriptions` (timing under a loaded machine); it passes alone (`1 passed`). Unrelated to this change.

## Smoke boot (real binary, fresh `ALLTERNIT_DATA_DIR`, port 18991)
```
applying migration: V215__voice_calls ...
POST /api/v1/voice/calls (unsigned)       -> HTTP/1.1 401  {"error":"missing signature"}
POST ... with sig headers (no token held) -> HTTP/1.1 503
panic/overlap lines in boot log: 0
refinery history top: 215|voice_calls
```
The existing `/api/v1/voice/voices` proxy still routes (401 from Clerk as before). No path overlap: the new routes are merged at root in the public router and share no path+method with `v1_routes.rs`'s `/voice/*` proxies.

## check_links
`checked 421 nav entries, 418 pages: 0 problem(s)`

## Device-token finding (the gap)
allternit-api never holds its own device token. `connector_routes::verify_runtime_device_token` only introspects tokens that *callers* present against cloud-api; the runtime's own `allternit_runtime_…` token lives in gizzi's env (`ALLTERNIT_API_TOKEN`) and cloud-api's `runtime_devices`. There is no `runtime_pairing.rs` in this repo (it's in cloud-api). So the verifier is written against the `VoiceRelaySecret` trait (`device_token()`, `paired_owner()`), and production uses `UnconfiguredRelaySecret` (both `None`): unsigned -> 401, signed -> 503 "voice relay not configured". To finish: a pairing step that hands this process its token and paired owner, then replace `UnconfiguredRelaySecret` in `VoiceDeps::production()`. Needs a joe-07/Eoj decision on how the token reaches the process.

## Left / known limits
- Device-token wiring (above). Until then calls don't work end to end.
- The turn path (`send_bot_turn`, same as channels) returns only the final reply, so `text.delta`s are sentence chunks of a whole reply and `tool` events are never emitted in production (the stream shape supports them; the test double proves it). True streaming and tool events need a streaming variant of the gizzi turn.
- The spoken-style preface is prepended to each turn's text, so it also appears in the session's user messages (not in the thread's `bot_events`).
- On abort, a vendor-bound session cancels on the vendor; otherwise `POST /v1/session/{id}/abort` on gizzi (what Stop does). Placed (remote) sessions aren't aborted remotely, same as Stop today.
- Not deployed. A merge deploys `cmd/allternit-api`; harmless until the token is wired (routes 401/503).
