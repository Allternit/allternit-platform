# Track E: runtime side of phone calls, `cmd/allternit-api/src/voice_calls.rs`

You are a **headless** Claude executor; nobody can steer you mid-run, so everything you need is here. The orchestrator (Claude session joe-9e, the "voice session") reviews your branch and merges it after Eoj's OK. **A merge of `cmd/allternit-api` deploys to production**, so quality matters.

Read first:
- `~/Desktop/allternit-workspace/HANDOFF-realtime-voice-2026-10-02.md` §4.1 (FROZEN call contract, including the 2026-10-02 control additions);
- `~/Desktop/allternit-workspace/AGENTS.md` (fix-it-now, docs with the feature, production mindset, Rust build cap);
- memory rule: smoke-boot allternit-api on a fresh db before any route merge; namespace new routes; check migration numbers (a route overlap crash-looped prod on 2026-09-30).

## Rules (hard)

- Work only in this worktree (`allternit-ao-voice-runtime`, branch `ao/voice-runtime`, off origin/main). Never run git in `~/Desktop/allternit-workspace/allternit` (the shared checkout).
- Use the default cargo config (it's capped machine-wide). **Don't set RUSTC_WRAPPER or CARGO_BUILD_JOBS.** Use `CARGO_TARGET_DIR=$HOME/Desktop/allternit-workspace/.voice-target-e` and **rm -rf it at the end**.
- **Don't edit joe-07's files:** `cmd/allternit-api/src/channel_*.rs` (including `channel_phone.rs`, which you CALL), `cmd/allternit-cloud-api/**`. You may add a single `.merge(...)`/`.nest(...)` line where routers are assembled, plus a `mod` line.
- Commit small and often, push the branch at the end. **Don't open or merge a PR.** Keep `docs/*_TASK.md`, `docs/*_NOTES.md` and sentinels out of feature commits (commit them separately at the end).

## What to build: runtime routes that joe-07's cloud-api relays calls to (shape agreed by both sessions)

1. **Auth extractor `RelayedVoiceAuth`** (in voice_calls.rs). It applies to these routes only.
   - Headers:
     - `x-allternit-runtime-sig: v1=<hex HMAC-SHA256(device_token, "<ts>.<METHOD>.<path>.<hex sha256(body)>")>`
     - `x-allternit-runtime-ts: <unix seconds>`
     - `x-allternit-owner: <userId>`
   - Verify with a constant-time compare, reject outside ±300 s, and treat the request as that owner (the owner must match the runtime's paired owner).
   - Find how this runtime knows its own device token (see `runtime_pairing.rs` / `authenticate_runtime_token`). If allternit-api can't read its own device token, STOP that part: implement the verifier against a `VoiceRelaySecret` trait, record the gap precisely in the notes, and leave the routes returning 503 "voice relay not configured". **Never accept unsigned requests.**
   - Body hashing needs the raw bytes: use a body-bytes extractor, then parse JSON.
2. **`POST /api/v1/voice/calls`**
   - Body: `{callId, botId, ownerId, numberId, from, to, direction, room, startedAt}`.
   - Resolve the thread with `channel_phone::resolve_thread_async(db, rt, numberId, caller)` (caller = `from` for inbound, `to` for outbound).
   - Store a row in a new `voice_calls` table: call_id PK, owner_id, bot_id, number_id, thread_id, session_id, from_e164, to_e164, direction, room, state, started_at, ended_at, created_at.
   - Return `{threadId, sessionId}`. Idempotent: a repeated callId returns the same ids.
   - **Migration:** next free `V<n>__voice_calls.sql`. List `cmd/allternit-api/migrations/` on origin/main right before you commit, and again after `git fetch && git rebase origin/main` at the end; renumber if someone took your number.
3. **`POST /api/v1/voice/calls/{callId}/events`**
   - Body: `{events:[{type, n, payload, occurredAt}]}`.
   - Write each event as a `bot_events` row on the call's thread via the same ledger helper channels use (`gateway_runner::led` or `thread_routes` ledger; follow the existing pattern).
   - Use `event_type = type` (the `call.*` names from §4.1), idempotency key `call:<callId>:<type>:<n>`, and actor `("caller", e164)` / `("bot", botId)` / `("human", userId)` from `payload.speaker`, or the bot for system events.
   - Every payload carries `callId`.
   - Only `call.transcript.delta` with `final:true` is persisted; ignore `final:false` (interim belongs to the live stream).
   - `call.ended` updates `voice_calls.state` / `ended_at`.
   - Reject events for unknown callIds (404).
4. **`POST /api/v1/voice/calls/{callId}/turn`**
   - Body: `{text, segmentId}`.
   - Run a bot turn in the call's session through the **same path channel messages use** (`gateway_runner::run_turn` and whatever it needs), so tools, memory and approvals behave as they do for channels. Tell the bot it's on a live phone call with a short spoken-style preface, kept server-side, so replies stay short and speakable.
   - Stream the reply as SSE:
     - `{"type":"text.delta","text":…}`
     - `{"type":"tool","name":…,"status":"started|done|error"}`
     - `{"type":"done"}`
     - `{"type":"error","message":…}`
   - One in-flight turn per call; a new turn while one is running aborts the old one first.
5. **`DELETE /api/v1/voice/calls/{callId}/turn`:** abort the in-flight turn (barge-in), using the same abort the Stop button uses. Return 204 even if nothing was running.
6. **Namespacing:** check that no existing route overlaps `/api/v1/voice/calls*` (there are existing `/api/v1/voice/*` proxies in `v1_routes.rs`). An overlap panics axum at boot.
7. **Tests:**
   - unit tests for the HMAC verifier (good, bad, stale and future ts; wrong owner; body tamper);
   - route tests on a fresh test db: create the call (idempotent), events (ordering, idempotency, interim ignored, unknown call 404), turn streaming with a fake ThreadRuntime (follow the existing test doubles in `gateway_runner.rs` tests), abort.
   - Then `cargo test -p allternit-api voice_calls` and the full `cargo test -p allternit-api --lib`.
8. **Smoke boot (required):** build the real `allternit-api` binary and boot it on a fresh temp DB dir/port. Confirm it starts with no route panic and that migrations apply, then curl an unsigned `POST /api/v1/voice/calls` and confirm a 401, not a 404 or panic. Paste the output in the notes.
9. **Docs:**
   - `cmd/allternit-api/docs/` or the closest existing API doc location: the four routes and the signature scheme.
   - Mintlify `surfaces/docs/` page for runtime voice-call routes (add to nav), then `python3 surfaces/docs/scripts/check_links.py` until it reports 0 problems.

## Done = `docs/VOICE_RUNTIME_NOTES.md` with

- **status:** done | partial
- the commits and pushed branch
- the migration number
- every file touched
- test output, smoke-boot output and check_links output (last lines)
- the device-token finding
- anything left

Then run `touch docs/VOICE_RUNTIME_NOTES.sentinel`. If you're running low on budget, write the notes FIRST with what's done.
