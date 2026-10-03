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

## E.1 results (relay secret wired, verifier shared)
**Done.** The device-token gap above is closed.

Files:
- `cmd/allternit-api/src/relay_auth.rs` (new, `pub mod` in `lib.rs`): `RelaySecret` trait (+ `credentials()`), `EnvOrFileRelaySecret`, `UnconfiguredRelaySecret`, `RelayedAuth` extractor, `sign_relay`, `verify_relay`, header consts, `MAX_SKEW_SECS`, `unix_now`. The extractor reads an `Extension<Arc<dyn RelaySecret>>`.
- `cmd/allternit-api/src/voice_calls.rs`: verifier/extractor removed; `VoiceDeps.secret` is `Arc<dyn RelaySecret>`, production = `EnvOrFileRelaySecret::from_process_env()`; the router layers the secret extension. 503 message is now `relay not configured`.
- `infrastructure/provisioned-instance/init.sh`: step 3 captures `userId` from the exchange and writes `ALLTERNIT_RUNTIME_OWNER_ID` to `$ENV_FILE`.
- `cmd/allternit-api/docs/VOICE_CALLS.md`, `surfaces/docs/guides/voice-call-runtime-routes.mdx` ("Known gap" replaced by the token-source section). `check_links`: 0 problems.

Behaviour: env pair first (both required), else identity JSON (`$ALLTERNIT_RUNTIME_IDENTITY_PATH` or `~/.config/allternit/runtime-identity.json`), cached by (mtime, size); empty token/owner, past `expiresAt` or an unparseable `expiresAt` -> None (503). Empty/absent `expiresAt` counts as fresh, as in allternit-node.

Tests: `cargo test -p allternit-api --lib -- relay_auth voice_calls`: `14 passed; 0 failed` (3 verifier tests moved; 4 new: env wins, file + default path, mtime rotation, expired/missing/corrupt/unparseable/no-owner).

Smoke (real binary, fresh data dir, identity file with owner `user-smoke`):
```
right owner  -> 400 {"error":"invalid body: missing field `botId`..."}   (past auth; my probe body was incomplete)
wrong owner  -> 401 owner does not match this runtime
wrong token  -> 401 bad signature
no file      -> 503 relay not configured
file restored-> 400 (picked up again without a restart)
panics in boot log: 0
```

init.sh finding: the pairing exchange response (`RuntimeSessionResponse` in `cmd/allternit-cloud-api/src/routes/runtime_pairing.rs`) does carry `userId`, so the owner is sourced from it. If it were ever empty, init.sh logs a WARNING and continues (routes would 503). Both the systemd unit (`EnvironmentFile=$ENV_FILE`) and the restart-loop runner (`. /etc/allternit-node/env`) already load `$ENV_FILE`, so no supervisor change was needed. Instances already paired (step 3 marker present) keep the old env file and need `ALLTERNIT_RUNTIME_OWNER_ID` added by hand or a re-pair.

Not deployed. A merge deploys `cmd/allternit-api`.
