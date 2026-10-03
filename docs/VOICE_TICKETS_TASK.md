# Track G: cloud voice tickets + voice minutes metering (cloud-api), headless

You are a **headless** Claude executor; nobody can steer you, so everything is here. The orchestrator (voice session joe-9e) reviews and merges. **Merging `cmd/allternit-cloud-api` auto-deploys cloud-api to production, and prod pg migrations are applied by hand as postgres.** Be careful and test properly.

Read:
- `~/Desktop/allternit-workspace/AGENTS.md` (fix-it-now, docs with the feature, production mindset, Rust build cap: don't set RUSTC_WRAPPER/CARGO_BUILD_JOBS);
- `~/Desktop/allternit-workspace/HANDOFF-realtime-voice-2026-10-02.md` §8 / §8.1 (pricing: Cloud Voice $0.08/min, 100 min/month included in Plus; phone $0.08/min + $2/month per number).

## Rules (hard)

- Worktree `allternit-ao-voice-tickets`, branch `ao/voice-tickets`, off origin/main. Never run git in `~/Desktop/allternit-workspace/allternit`.
- `CARGO_TARGET_DIR=$HOME/Desktop/allternit-workspace/.voice-target-g`; **rm -rf it at the end.**
- **New files only**, plus the minimal router/mod/migration registration lines: `cmd/allternit-cloud-api/src/routes/voice_tickets.rs`, `src/voice_usage.rs` (or `services/voice_usage.rs`, following the crate's layout), and a migration.
- Don't edit joe-07's files: `routes/channel_inbound.rs`, `routes/phone*`, `routes/whatsapp*`, `routes/voice_calls*` / the voice-cloud routes, `channel_*`.
- **No Stripe calls, no charges, no usage records in Stripe.** Meter and expose only (money actions stay human-approved).
- **Don't invent a second plan source.** Use the existing `state.quota_service` / `QuotaService` (and whatever plan lookup `routes/channel_inbound.rs` and `runtime_relay.rs` use) to decide the user's plan.
- Commit small, push the branch; no PR or merge. Keep `docs/*_TASK.md` / `*_NOTES.md` / sentinels in a separate final commit.

## Build

1. **Pricing constants in ONE place** (`voice_usage.rs`):
   - `PLUS_INCLUDED_CLOUD_VOICE_SECONDS = 100*60`;
   - `CLOUD_VOICE_RATE_USD_PER_MIN = 0.08`;
   - `PHONE_RATE_USD_PER_MIN = 0.08`;
   - `PHONE_NUMBER_USD_PER_MONTH = 2.0`.
   - The free plan's included cloud minutes: read how plans are modelled; if free has no cloud voice, it gets 0 included and gets the "no cloud minutes" answer.
   - Make these `pub`, so joe-07 reuses them for the phone path.
2. **Migration:**
   - The next free cloud pg number **≥ 029**. 022, 023, 026 and 027 are pending on other branches; 024, 025 and 028 are taken. Check `cmd/allternit-cloud-api/migrations/` on origin/main right before committing, and again after a final `git fetch && git rebase origin/main`.
   - Table `voice_usage`: id, user_id, engine (`cloud` | `phone`, CHECK), seconds INT, ref TEXT (session or call id), occurred_at timestamptz, created_at. Unique (engine, ref) for idempotency, plus an index on (user_id, occurred_at).
   - Table `voice_tickets_used`: nonce PK, user_id, expires_at, used_at. Index expires_at; old rows are purged opportunistically.
   - Follow the existing migration style exactly (look at 021/024).
3. **`pub async fn record(db, user_id, engine, seconds, ref)`**: idempotent on (engine, ref); returns Ok on a duplicate. Add a `month_usage(db, user_id, engine)` helper too.
4. **`POST /api/v1/voice/tickets`** (Clerk-authenticated user; use the same auth extractor other user routes in cloud-api use):
   - Look up the plan via the existing machinery and compute the remaining cloud minutes for the current calendar month (UTC).
   - If there's no allowance and the plan can't go into overage (free) → 402 `{code:"no-cloud-minutes"}`.
   - Otherwise mint `ticket = "v1." + b64url(json{sub,plan,exp,nonce,maxSeconds}) + "." + b64url(HMAC-SHA256(ALLTERNIT_VOICE_TICKET_SECRET, "v1."+payload))`, where exp is now+60 s and maxSeconds caps the session (a sensible max, e.g. 30 min).
   - Return `{ticket, wsUrl, expiresAt}` with `wsUrl = env ALLTERNIT_VOICE_CLOUD_WS_URL` (e.g. wss://voice.allternit.com/v1/voice/session).
   - Missing secret or URL env → 503 `{code:"cloud-unavailable"}`. Never mint unsigned.
5. **`POST /api/v1/voice/tickets/redeem`** (the voice service calls this; bearer `ALLTERNIT_VOICE_WORKER_TOKEN`, constant-time compare): body `{ticket}` → verify the HMAC + exp, then insert the nonce into `voice_tickets_used` (single use) → `{sub, plan, maxSeconds}`, or 401. (The voice service can also verify the HMAC locally; this endpoint gives single-use enforcement.)
6. **`POST /api/v1/voice/usage`** (bearer `ALLTERNIT_VOICE_WORKER_TOKEN`): `{sub, sessionId, seconds, engine:"cloud"}` → `record`. Idempotent.
7. **`GET /api/v1/voice/usage/month`** (Clerk user) → `{engine:"cloud", usedSeconds, includedSeconds, overageRateUsdPerMin, phone:{usedSeconds, rateUsdPerMin}}`.
8. **Route namespacing:** make sure nothing else in cloud-api already serves `/api/v1/voice/tickets*` or `/api/v1/voice/usage*`; joe-07's voice-cloud branch serves `/api/v1/voice/calls*`. An axum overlap panics at boot.
9. **Tests:**
   - ticket mint/verify: good, tampered, expired, wrong secret;
   - single-use redeem;
   - idempotent record;
   - month math across the month boundary;
   - 402 for no allowance; 503 when unconfigured.
   - Use the crate's existing test patterns and test DB approach (look at the existing route tests). Run `cargo test -p allternit-cloud-api` (relevant filters plus the full lib tests).
10. **Boot check:** build the cloud-api binary and boot it the way existing docs/tests do (if it needs Postgres and none is available locally, say so in the notes and rely on the route-construction test that builds the full router without panicking; write that test if it doesn't exist).
11. **Docs:** a Mintlify page `surfaces/docs/api/voice-tickets.mdx` (tickets, usage endpoints, env vars, pricing constants) in the nav next to `api/voice`. Run `python3 surfaces/docs/scripts/check_links.py` until it reports 0 problems. List the new env vars in the cloud-api env example/README if one exists.

## Done = `docs/VOICE_TICKETS_NOTES.md` with

- **status**
- the commits and pushed branch
- the migration number
- the files
- test output
- the boot-check result
- the env vars to set in prod: `ALLTERNIT_VOICE_TICKET_SECRET`, `ALLTERNIT_VOICE_CLOUD_WS_URL`, `ALLTERNIT_VOICE_WORKER_TOKEN`
- the exact SQL to run by hand in prod

Then run `touch docs/VOICE_TICKETS_NOTES.sentinel`. If you're low on budget, write the notes first.
