# Voice call routes (runtime side)

Source: `cmd/allternit-api/src/voice_calls.rs`, migration `V215__voice_calls.sql`. Developer guide with the full contract: `surfaces/docs/guides/voice-call-runtime-routes.mdx`.

cloud-api relays a phone call's start, its events and its bot turns to the runtime. The routes are public to the Clerk middleware and authenticate themselves.

| Route | Purpose |
| --- | --- |
| `POST /api/v1/voice/calls` | Start. Body `{callId, botId, ownerId, numberId, from, to, direction, room, startedAt}`. Returns `{threadId, sessionId}`. Idempotent per `callId`. |
| `POST /api/v1/voice/calls/{callId}/events` | Body `{events:[{type, n, payload, occurredAt}]}`. Writes `call.*` events to the call's thread. |
| `POST /api/v1/voice/calls/{callId}/turn` | Body `{text, segmentId}`. Runs a bot turn, streams SSE. |
| `DELETE /api/v1/voice/calls/{callId}/turn` | Barge-in. Always 204. |

## Signature

```
x-allternit-runtime-sig: v1=<hex HMAC-SHA256(device_token, "<ts>.<METHOD>.<path>.<hex sha256(body)>")>
x-allternit-runtime-ts:  <unix seconds, within ±300 s>
x-allternit-owner:       <userId, must equal the runtime's paired owner>
```

`path` is the request path without the query string. The body is hashed as received (empty for `DELETE`). Missing or bad headers: 401. A signed request on a runtime that cannot read its device token: 503 `relay not configured`. Unsigned requests are never accepted.

## Device token source

Verification lives in `src/relay_auth.rs` (shared with other relayed envelopes). The production secret, `EnvOrFileRelaySecret`, reads in this order:

1. env `ALLTERNIT_RUNTIME_DEVICE_TOKEN` and `ALLTERNIT_RUNTIME_OWNER_ID` (both must be set);
2. the identity JSON at `$ALLTERNIT_RUNTIME_IDENTITY_PATH`, default `~/.config/allternit/runtime-identity.json` (keys `deviceToken`, `userId`, `expiresAt`; the file allternit-node and agent-daemon share). It is re-read when the file's mtime or size changes, since the token rotates. A past `expiresAt` counts as absent.

With neither, signed requests answer 503 `relay not configured`. Provisioned cloud computers get both env vars from `init.sh` (written to `/etc/allternit-node/env`, loaded by the systemd unit and the restart-loop runner). The desktop `ALLTERNIT_API_TOKEN` is a Clerk session token, not the device token, and is never used.
