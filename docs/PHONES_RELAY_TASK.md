# WORKTREE CORRECTION

The real git worktree is the NESTED path, not this directory. Find it with: `git -C allternit worktree list | grep phones` — work ONLY there. This directory only holds this task file.

# Phones 2.0 — Phase 2 platform: phone stream relay (Rust, allternit-api)

You are one of four parallel lanes building Phones 2.0 (approved plan, 2026-10-17). Yours is the **relay lane**: the browser-facing WebSocket that lets any surface (ai.allternit.com phone layout, m.allternit.com PWA) view and control a linked Android through the user's Desktop app. You mirror the existing computer-lane proxy minus VNC — the payload is the scrcpy H.264 stream and JSON control envelopes, which you treat as opaque.

## Where you are

- Worktree: `/Users/joe/Desktop/allternit-workspace/allternit-wt-phones-relay`, branch `session/phones-relay` (from current origin/main).
- Never edit any other checkout. Commit on `session/phones-relay` in logical commits. Do NOT push, merge, or open PRs.
- Repo rules: read `/Users/joe/Desktop/allternit-workspace/AGENTS.md` and this repo's AGENTS.md. pnpm only; **cargo always with `CARGO_TARGET_DIR=/Users/joe/Desktop/allternit-workspace/.shared-target/allternit-wt-phones-relay`** (machine-wide build caps; never touch `.shared-target` root itself). Prefer `cargo check` while iterating; full `cargo test -p allternit-api` before finishing. Port 8013 belongs to the installed Desktop — never set `ALLTERNIT_API_PORT=8013`; the default 18013 is correct.

## Read first

1. `cmd/allternit-api/src/computer_ws.rs` — the model: `sign_computer_token`/claims validation (~line 1946 for test usage), `validate_vnc_ws_request` (~346), the WS route + proxy loop (~754), the ws-token mint route (~1384). Reuse its helpers where they are generic; write phone-specific ones beside them.
2. `cmd/allternit-api/src/computer_embed.rs` (embed token purpose), `cmd/allternit-api/src/vnc_readonly.rs` (read-only enforcement shape).
3. `cmd/allternit-cloud-api/src/routes/paired_computers.rs` — how account-level pairing rows live in cloud-api (Postgres) and how allternit-api reaches cloud-api.
4. `cmd/allternit-cloud-api/migrations_pg/` — the migration naming/numbering convention (pick the next number).
5. How Desktop authenticates to allternit-api elsewhere (grep `desktop_secret`, `x-allternit-desktop`): the phone-link session WS reuses that credential pattern.

## The contract (a sibling frontend lane codes against this verbatim — do not deviate)

HTTPS (Clerk-authenticated, existing auth extractors):

- `GET /api/v1/phones` → JSON `{ "phones": [{ "serial": string, "model": string, "name": string, "online": boolean }] }` — rows registered to the caller's account in cloud PG; `online` = an attached phone-link session exists in this api instance's registry.
- `POST /api/v1/phones/:serial/ws-token`, body `{ "readOnly"?: boolean }` → `{ "token": string, "expiresInSec": number }`. Minting requires: the phone row exists for this account AND a Desktop session is attached. TTL 300 s, HMAC-signed, claims: `sub` (account), `serial`, `purpose: "phone_stream"`, `read_only` (default TRUE — a full-control token injects input into a personal phone; mint it only for the same explicit-request pattern computer_ws uses, defaulting read-only).

WebSocket routes:

- `/ws/phones/:serial/stream?token=…` (public mount, token-gated like the computer vnc route): validate token → find the account's attached Desktop session → proxy frames bidirectionally. Frames: JSON text and binary, passed through untouched (the desktop session handles semantics). Socket close 4401 with a JSON `{"type":"end","reason":"token_expired"}` if the token crosses expiry mid-stream — compute the remaining TTL at upgrade and timer-close at it.
- `/ws/phone-link/session` (desktop-attached, desktop-credential auth): on connect the client sends one JSON text frame `{"type":"hello","phones":[{"serial","model","name"}]}`; register (account resolved from the credential, phones upserted in cloud PG with `last_seen_at`, registry marks online); heartbeat ping/pong (30 s); any frame from the attached browser socket for `(account, serial)` is forwarded to this socket and vice versa, order preserved per direction, backpressure handled (if one side stalls > 5 s, apply `tokio::select` fairness + bounded channel, drop video binary frames rather than control JSON). On disconnect: mark offline, update `last_seen_at`, buffered browser waiters get `{"type":"end","reason":"desktop_offline"}`.

Cloud-api (Postgres):

- Migration `migrations_pg/0XX_registered_phones.sql`: `registered_phones (id uuid pk default gen_random_uuid(), account_id text not null, serial text not null, model text, name text, created_at timestamptz default now(), last_seen_at timestamptz, unique(account_id, serial))` + index on account_id. (Check the actual latest migration number; gen_random_uuid availability per existing migrations — match their uuid pattern.)
- A cloud-api route for allternit-api to upsert/list by account, following `paired_computers.rs` precedent (same crate layout, same client pattern allternit-api already uses).

## Registration model (decided)

The registry is in-memory in allternit-api (`HashMap<(account_id, serial), SessionHandle>`); persistence is only the cloud PG `registered_phones` rows (device identity + last_seen), so `GET /api/v1/phones` can report "linked but Desktop offline". No screen data, no input data is ever persisted or logged beyond normal access logs.

## Tests

Follow `computer_ws.rs`'s in-file test module style (it builds an app state with a test secret — copy that): token sign/verify + claims enforcement (wrong account, expired, read_only passthrough), registry attach/detach/heartbeat, relay pass-through with a mocked desktop socket (tokio duplex), 4401 expiry close, hello-frame validation, `GET /api/v1/phones` with a stubbed cloud client. Run `cargo test -p allternit-api` with the shared target dir above. Also `cargo build -p allternit-api` release-profile check if you touched anything in the shipping path.

## Out of scope

`surfaces/allternit-desktop/**` entirely (a sibling lane owns it — the Desktop-side session client that connects to `/ws/phone-link/session` is the orchestrator's integration step after both lanes land). Frontend. Anything Telnyx.

## Done means

Sentinel: write `docs/PHONES_RELAY_NOTES.md` (file list, route table, token claims, registry behavior, test output summary) and `touch docs/PHONES_RELAY.sentinel`.
