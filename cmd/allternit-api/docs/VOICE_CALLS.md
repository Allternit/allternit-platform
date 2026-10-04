# Voice call routes (runtime side)

Source: `cmd/allternit-api/src/voice_calls.rs`, migration `V218__voice_calls.sql`. Developer guide with the full contract: `surfaces/docs/guides/voice-call-runtime-routes.mdx`.

cloud-api relays a phone call's start, its events and its bot turns to the runtime. The routes are public to the Clerk middleware and authenticate themselves.

| Route | Purpose |
| --- | --- |
| `POST /api/v1/voice/calls` | Start. Body `{callId, botId, ownerId, numberId, from, to, direction, room, startedAt}`. Returns `{threadId, sessionId}`. Idempotent per `callId`. If `numberId` is unknown on this runtime, it is registered from the signed body (bot number = `to` inbound, `from` outbound); a bot that isn't the owner's returns 422. `call.ended` carries `answered` / `missed` (see CALL_WORKER.md §4.1). |
| `POST /api/v1/voice/calls/{callId}/events` | Body `{events:[{type, n, payload, occurredAt}]}`. Writes `call.*` events to the call's thread. |
| `POST /api/v1/voice/calls/{callId}/turn` | Body `{text, segmentId}`. Runs a bot turn, streams SSE (`text.delta`, `tool`, `done`, `error`). |
| `DELETE /api/v1/voice/calls/{callId}/turn` | Barge-in. Always 204. |

## Signature

```
x-allternit-runtime-sig: v1=<hex HMAC-SHA256(relay_key, "<ts>.<METHOD>.<path>.<hex sha256(body)>")>
x-allternit-runtime-ts:  <unix seconds, within ±300 s>
x-allternit-owner:       <userId, must equal the runtime's paired owner>
```

`path` is the request path without the query string. The body is hashed as received (empty for `DELETE`). Missing or bad headers: 401. A signed request on a runtime that cannot read its device token: 503 `relay not configured`. Unsigned requests are never accepted.

The HMAC key is `relay_key = sha256_hex(device_token)`: the ASCII bytes of the lowercase hex digest, not the raw token. cloud-api only stores that digest as `credential_hash`, so it can sign without the raw token. The message format is unchanged. A signature made with the raw token is rejected.

Known-answer vector (device token `tok-123`, ts `1700000000`, `POST`, path `/api/v1/voice/calls`, body `{}`):

- `relay_key` = `c8963414bf6c4c869eeac5f8a057c3dc574d422f1b108397b66f67bab3d2f981`
- signature = `34edb38cb1c7839397d5993a055972d2f352b42164bcdfd935264cdcae1d1576` (send as `v1=34edb38cb1c7839397d5993a055972d2f352b42164bcdfd935264cdcae1d1576`)

The credential hash is now a signing secret for relays: treat `runtime_devices.credential_hash` as secret. Anyone with database access can forge relayed requests; runtimes are only reachable through the cloud relay.

## Device token source

Verification lives in `src/relay_auth.rs` (shared with other relayed envelopes). The production secret, `EnvOrFileRelaySecret`, reads in this order:

1. env `ALLTERNIT_RUNTIME_DEVICE_TOKEN` and `ALLTERNIT_RUNTIME_OWNER_ID` (both must be set);
2. the identity JSON at `$ALLTERNIT_RUNTIME_IDENTITY_PATH`, default `~/.config/allternit/runtime-identity.json` (keys `deviceToken`, `userId`, `expiresAt`; the file allternit-node and agent-daemon share). It is re-read when the file's mtime or size changes, since the token rotates. A past `expiresAt` counts as absent.

With neither, signed requests answer 503 `relay not configured`. Provisioned cloud computers get both env vars from `init.sh` (written to `/etc/allternit-node/env`, loaded by the systemd unit and the restart-loop runner). The desktop `ALLTERNIT_API_TOKEN` is a Clerk session token, not the device token, and is never used.

The same signature (verified with `relay_auth::RelayedAuth`) now guards every other runtime path that trusts cloud-api: `/webhooks/channels/discord-app`, `/webhooks/channels/slack-app`, `/webhooks/teams-app`, `/api/v1/agent-email/inbound` and the Telegram managed connect route. See "Other signed relays" in the developer guide.

## Streaming turns

`voice_turn_stream.rs` streams a native Gizzi session: it subscribes to `/event` before sending the same message the channel path sends (`native_turn_request`), then folds `message.part.delta` and `message.part.updated` (`part.type == "tool"`) events for that session into `text.delta` (whole words) and `tool` (`started|done|error`) events. Tool steps are written to the thread as `agent.tool.*` by the call route (the native channel path writes none, so nothing is doubled). A `permission.asked` event aborts the turn and answers a spoken error, because nobody can approve on a call. Turns are capped at 120 s. `DELETE /turn` cancels the turn task and aborts the Gizzi session. Vendor-bound and placed sessions fall back to the channel path (final reply, no tool events) with an info log line `voice turn: session is ...`. On a phone call the caller hears spaced fillers while that final reply is pending (see services/voice/spec/CALL_WORKER.md); the call worker and cloud-api allow 60 s for the first reply byte.

## Call summaries

When a `call.ended` event lands for a call that has at least one final transcript segment, the runtime writes a `call.summary` event to the thread in the background: `{callId, text, followUps: [..]}`, idempotency key `call:<callId>:call.summary:0`, actor the bot. The text covers who called, what they wanted, what the bot did (tool steps included) and the follow-ups for the owner.

It is generated with the bot's own model (`bot_turn_model`) in an ephemeral gizzi session that is deleted afterwards (`structured_output::complete_structured`, schema-constrained, tools off), so it never touches the call's session or the thread's history. Input is the call's own ledger rows (transcript lines and `agent.tool.*` steps, capped at 24k characters). A call with no final segment (missed, silent) gets no summary. A failure or a 150 s timeout is logged and dropped: no event, no retry. The seam is `CallSummarizer` on `VoiceDeps`; tests use a fake.

`call.transcript.revised` (`{callId, segments:[{text, startMs, endMs}]}`) is posted by the voice worker after a recorded call; the runtime writes it like any other `call.*` event. See `services/voice/spec/CALL_WORKER.md`.
