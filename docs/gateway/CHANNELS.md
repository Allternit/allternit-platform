# Channels

**What this is:** how Slack, Teams, Discord and WhatsApp conversations become Threads, and how replies are posted back.
**Who it's for:** engineers working on channel transports or channel-origin threads.
**Last verified against:** platform commit `c3af0730ca`, allternit-ai commit `88f4d6f6`.

Spec: `Allternit Brain/Research/specs/channel-packs.md`. Related: [ARCHITECTURE.md](ARCHITECTURE.md), [CONNECTIONS_AND_CREDENTIALS.md](CONNECTIONS_AND_CREDENTIALS.md). Code: `cmd/allternit-api/src/{channel_gateway,channel_transports,teams_auth,discord_gateway}.rs`.

## Model

One Thread is bound to one external conversation through a `channel_conversation_bindings` row (V198). An append-only `channel_message_log` (V200) holds stable remote ids, correlation ids and dedupe state. Channel activity is written to the bot ledger as `channel.*` events tagged with the thread (`channel.message.received, channel.message.sent, channel.reaction.updated, channel.message.edited, channel.message.deleted`).

Binding fields: `provider, account_binding_id, external_workspace_id, external_channel_id, external_conversation_id, external_thread_id, canonical_url, bidirectional, read_only, posting_identity_id, last_inbound_cursor, last_outbound_cursor, sync_state`. Sync states in Rust: `LIVE, DELAYED, RECONNECTING, DEGRADED, DISCONNECTED`. (The TS contract enum differs, see [ARCHITECTURE.md](ARCHITECTURE.md#2-object-model).)

The conversation key is stable per provider, for example `slack:<channel>:<root ts>`. Every inbound event ends in `send_bot_turn` when it should run a bot turn, so channels are one more way into the same runner.

## ChannelTransport

Defined in `channel_gateway.rs`. Every platform implements it:

| Method | Purpose |
|---|---|
| `provider()` | Provider name |
| `verify(secret, headers, body)` | Verify the platform signature or token on an inbound webhook |
| `normalize(payload)` | Payload to zero or more `Inbound` events. Unknown shapes yield none. |
| `identity(requested)` | Whether the platform lets us post as exactly this identity, or only as the app on the bot's behalf (`relayed`) |
| `post(outbound)` | Send. Returns a `Receipt`, or `PostError`: definite rejection (nothing posted) or uncertain (timeout, transport error, 5xx) |
| `fetch_since(channel, thread, cursor)` | Events after a cursor, for reconnect. Default returns none. |

`Inbound` carries a stable conversation key, the platform thread id, a stable event id (unique per message, edit version or reaction change), the id of the message the event is about, a monotonic cursor (Slack ts, Discord snowflake, WhatsApp timestamp), and flags for own-echo and relayed posts. Platform-specific parsing is in pure functions (`teams_normalize`, `discord_normalize`, `whatsapp_normalize`), tested offline against each platform's documented payloads. The only I/O is the injected `HttpSend`.

Recording and resume:

- `record_inbound` writes an event once. Replays and Slack retries are no-ops. Our own message coming back confirms a pending post.
- `resume` reconnects: it pulls everything after `last_inbound_cursor` and records it. Replays are no-ops.
- Cursors compare numerically where possible (`cursor_after`).

## Providers

| Provider | Inbound | Outbound | Secret shape (sealed JSON in `secret_ref`) |
|---|---|---|---|
| Slack | Existing `slack_webhook_routes` (signed, public route); `verify_slack_signature` | `chat.postMessage` with `ALLTERNIT_SLACK_BOT_TOKEN`; `ALLTERNIT_SLACK_BOT_USER_ID` identifies our own posts | `SlackTransport::from_env()` takes token and user id from env, not from account secrets |
| Teams | `POST /webhooks/channels/teams` | Bot Framework or outgoing-webhook reply | `{ securityToken, accessToken }` (outgoing-webhook HMAC plus static bearer), or `{ appId, appPassword }` (Bot Framework JWT in, client-credentials token out) |
| Discord | `POST /webhooks/channels/discord` (Ed25519 interactions) and the Gateway websocket | Channel webhook | `{ publicKey, webhookUrl }`, plus `botToken` to start the websocket client |
| WhatsApp | `POST /webhooks/channels/whatsapp`, and `GET` for Meta's subscribe handshake | WhatsApp Business Cloud API | `{ appSecret, verifyToken, accessToken, phoneNumberId }` |

Verification per provider:

- Teams outgoing webhook: `Authorization: HMAC <base64(HMAC-SHA256(base64decode(securityToken), body))>`.
- Teams Bot Framework: `teams_auth.rs` validates the bearer JWT against Microsoft's OpenID metadata and JWKS. It checks RS256 only, issuer, audience (the bot's app id), expiry and not-before, and that the `serviceurl` claim matches the activity's `serviceUrl`. JWKS is cached and refetched on an unknown `kid` (rate limited) or when stale. Outbound tokens are cached until shortly before expiry. One `TeamsAuth` exists per app id per process.
- Discord: Ed25519 over `timestamp + body` with `publicKey`.
- WhatsApp: `X-Hub-Signature-256: sha256=<hex HMAC-SHA256(appSecret, raw body)>`.

The generic webhook (`webhook_h`) returns 404 for `slack` and any provider outside the supported list. It tries each candidate account for the provider: Teams JWT first, then each account's `verify`. No match means the request is rejected. The route is public because platforms call it server to server.

Discord Gateway (`discord_gateway.rs`): started at boot for each Discord account whose secret has a `botToken` (`spawn_bound`, called from `main.rs`). It handles HELLO, heartbeat with ACK tracking (a missed ACK reconnects), IDENTIFY or RESUME with `session_id` and last `seq`, and passes MESSAGE_CREATE, UPDATE, DELETE and reaction dispatches on unchanged. Default intents include `MESSAGE_CONTENT`, which is privileged and must also be enabled in the Discord developer portal. `ALLTERNIT_DISCORD_GATEWAY=0` disables it. Accounts with no `botToken` start nothing, and the interactions webhook path is unaffected.

## Bindings and threads

- `POST /gateway/threads/:thread_id/channel-bindings` (owner-scoped, see [AAI_REST.md](AAI_REST.md#remote-thread-and-channel-bindings)) creates a binding. `GET` lists them. `PATCH /gateway/channel-bindings/:id` updates sync state and fields.
- A new inbound conversation becomes a bot thread only when its account is restricted to a bot. Otherwise the event is ignored (`route_inbound`).
- Replies are posted back after the runner produces one (`dispatch_events`, shared by the webhook and the Discord websocket).

## Send endpoint, policy and approval

`POST /gateway/threads/:thread_id/channel-send` (`channel_gateway_router`, authenticated). Body: `text`, optional `posting_identity_id`, `correlation_id`, `consequential`, `allternit_approval_id`.

Order of checks in `channel_gateway::send`:

1. Binding exists and is not `read_only`.
2. Bot policy from `channel_tools`: rule `channel.send` or `<provider>.send` with action `deny` (403 `CHANNEL_POLICY`) or `ask`.
3. Allternit approval for consequential posts, or when policy says `ask`: 428 with `approvalId`. See [ARCHITECTURE.md](ARCHITECTURE.md#7-approvals-two-authorities).
4. Replay check on `correlation_id`.
5. `transport.post`. The returned remote id is stored as `last_outbound_cursor`, and the send is logged in `channel_message_log`.

Responses:

| Outcome | HTTP | Body `code` |
|---|---|---|
| Sent | 200 | `state: sent`, `remoteId`, `relayed`, `correlationId` |
| Delivery uncertain | 202 | `state: unconfirmed`, label Pending. Never re-posted blindly. The inbound echo or a resume confirms it. |
| Needs approval | 428 | approval id |
| Policy denied | 403 | `CHANNEL_POLICY` |
| Read-only | 403 | `READ_ONLY` |
| No binding | 404 | `NO_BINDING` |
| Platform said no | 502 | `CHANNEL_REJECTED` |
| Same correlation id as an earlier send | 200 | `replay: true`, earlier state |
| Provider not configured | 503 | `CHANNEL_OFFLINE` |

Every outbound post is logged. When the platform cannot post as the exact identity, the message goes out relayed and says so (`relayed`, `posting_identity_id`).

## Muse lane

Muse (Meta) is reached over WhatsApp, so it is a `channel` lane, not a UI bridge. A bot whose execution binding uses a channel lane goes through `ChannelLaneTransport` (`channel_transports.rs`): the turn is posted over the account's channel transport and the vendor's replies, pulled by the lane transport, come back as `agent.message.completed` with who, whose and how taken from the binding. Replies on that conversation are the vendor's answers, never new user turns. Anything on another lane passes through to the normal transport. WhatsApp delivery statuses become receipts.

Tested offline by `muse_over_whatsapp_sends_and_returns_replies_as_agent_message_completed`. No `services/subscription-gateway` adapter exists for Muse. The web pack seed (`meta`, "Meta Muse", auth `channel_oauth`) exists, and no Muse look pack is registered.

## What needs real accounts

Everything above is tested against documented payload shapes and fake transports. None of it has run against a live platform:

| Needs | For |
|---|---|
| A real Slack workspace | Slack event to thread in under 3 seconds, reply in the right channel thread, resume after disconnect |
| A real Teams tenant and Azure bot registration | Bot Framework JWT validation against live Microsoft keys, outgoing tokens |
| A real Discord bot and application (privileged intent enabled) | Websocket session, interactions signature |
| A WhatsApp Business account and Meta app | Webhook signature, handshake, Muse conversation |

Those are recorded as blocked in [ACCEPTANCE.md](ACCEPTANCE.md). The steps and who has to consent are in [OPERATIONS.md](OPERATIONS.md#live-verification-checklist).
