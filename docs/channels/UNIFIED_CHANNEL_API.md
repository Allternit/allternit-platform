# Unified channel API: audit, contract and migration plan

Status as of 2026-10-13. Plan source: Allternit Brain `Research/specs/allternit-unified-channel-api.md`. Reference only: BerriAI AgentChat (not installed, nothing copied). Reference page for API users: `surfaces/docs/guides/channel-gateway-api.mdx`.

## Decision

There is no new gateway. The existing `ChannelTransport` trait and `channel_gateway` service in `cmd/allternit-api/src/channel_gateway.rs` are the unified API. This work extends them with explicit capabilities, typed rate limits, a delivery-status read, restart recovery for outbound posts, and owner/connection-scoped routing. It does not add new tables or migrations.

## Phase 1 audit (code evidence, base 4fe1cb4472)

### Providers registered

`channel_transports::PROVIDERS` = slack, teams, discord, whatsapp, telegram, whatsapp-personal, sms. `build_transport` selects one adapter per provider and secret shape:

| Provider | Adapter(s) | Ingress | Egress | Production status |
|---|---|---|---|---|
| slack | `SlackAppTransport` (shared app); `SlackTransport` (legacy env token) | Cloud verifies Slack's signature and queues events (`slack_event_queue`, retries with backoff) → signed relay → `channel_slack_app::shared_events_h`. Legacy: `slack_webhook_routes` | Cloud `/api/v1/channels/slack/send` (per-bot username/icon); legacy `chat.postMessage` | Shared app shows "Coming soon" until the production Slack app is set up. Unverified live |
| teams | `TeamsAppCloudTransport`; `TeamsTransport` (outgoing webhook / Bot Framework JWT) | `/webhooks/channels/teams` (JWT against Microsoft JWKS or HMAC) | Cloud send / Bot Framework | Unverified live |
| discord | `DiscordAppTransport` (shared app); `DiscordTransport` (webhook) | Gateway websocket (`discord_gateway.rs`) and interactions | Cloud send / webhook URL | Unverified live |
| whatsapp | `WhatsAppBusinessTransport`; `WhatsAppTransport` | `/webhooks/channels/whatsapp` (Meta signature; GET challenge) | Graph API / cloud | Unverified live |
| telegram | `TelegramTransport` | `/webhooks/channels/telegram` (secret token header) | Bot API, multipart files | Live via BotFather token; managed bots path gated |
| whatsapp-personal | `WhatsAppPersonalTransport` | bridge | bridge | Behind `channel_whatsapp_personal::enabled()` |
| sms | `SmsTransport` | Cloud phone service → runtime | Cloud `/api/v1/channels/sms/send`, multipart split | Live (phone service) |

Email (Allternit Mail) and iMessage (`allternit-openclaw-host/native_imessage_bridge.rs`) are outside `ChannelTransport` and were not migrated. Nothing here claims they need a new integration.

### Capability matrix

Implemented as `ChannelTransport::capabilities()`. Each value follows from the adapter's code: whether it implements `post_files` or `fetch_since`, whether its `normalize` emits reaction, edit or delivery kinds, whether `channel_attachments::inbound_files` handles the provider, and what its `identity()` returns. The table is in the reference page. Notable facts:

- Only the legacy Slack adapter implements `fetch_since` (history replay). The trait default used to return `Ok(vec![])`, which made "unsupported" look like "nothing missed". It now returns `Err("history_unsupported")`, and `resume` refuses adapters without `history`.
- Only WhatsApp emits delivery receipts (`InboundKind::Delivery`). On every other provider `confirmed` means only that the platform accepted the post.
- Inbound files are kept only for Slack and Telegram.

### Defects found and fixed in this change

1. **Cross-owner / cross-connection routing.** `ensure_binding_on` looked up `(provider, conversation)` with no owner, and `channel_conversation_bindings` has no unique key on it. Telegram keys private chats by the person's id. So when one person messaged two owners' bots, the second owner's message joined the **first owner's** binding: it was recorded on that owner's thread and ran a turn on that owner's bot (`route_inbound` filtered by owner, found nothing, created a thread, then `ensure_binding_on` returned the other owner's row). Two bots of one owner had the same problem between each other. WhatsApp personal (`whatsapp-personal:<jid>`) and the Discord shared app (`discord:<channel>`) had the same shape. Fixed with `find_binding_for(owner, account, provider, conversation)`, used by `ensure_binding_on`, `route_inbound`, `channel_start`, the WhatsApp control lane and `people`. A legacy binding with no account adopts the first connection it hears from, so no conversation is duplicated.
2. **Slack shared-app team lookup** picked the first connection on the runtime with that team id and then refused (`403`) when it belonged to a different owner. Each owner's event was refused for every owner but the first. Now it is scoped to the owner the cloud relay signed for.
3. **Stuck `pending` sends.** A crash between recording a send and the platform's answer left the row `pending` forever. `recover_interrupted_sends` runs at startup and marks them `unconfirmed`, with a thread event, and never re-posts.
4. **Rate limits were reported as definite refusals.** A `429` was `PostError::Rejected("rate limited")`, so the send was stored as `failed` and a retry with the same correlation id was blocked as a replay. There is now a `PostError::RateLimited { retry_after_secs }` variant. Its send outcome is stored as `rate_limited`, can be retried with the same correlation id, and is returned as HTTP 429 with `Retry-After`. SMS keeps its rule: a throttle on part 2+ of a split text is `Uncertain`, since part 1 already went out.

### Bypass paths (posts that don't go through `send`/`post_reply`)

System notices only, by design (see "System notices" in the reference page): Telegram pairing reply, Factory approval prompts/answers (including a direct Telegram `sendMessage` in `factory_approvals_channels`), sender-refusal notices, and the "no bot switched on" notice when the chat has no thread. They must stay out of bot routing. Notices in a chat that has a thread go through `post_reply` and are logged.

### Inbound durability (done)

Before: the cloud queues Slack/Discord/Teams/SMS events durably, but the runtime answered 2xx and then spawned the work, and provider webhooks did the same, so a crash between the ack and `record_inbound` lost the event. Now `accept_and_ack` records every event, stores its turn on the log row (`detail_json.turn`, queued → running → done), answers (500 on a recording failure, so the sender redelivers), and only then runs the turns. Turns of one session are serialized. `recover_channel_turns` at startup runs `queued` turns and marks `running` ones `interrupted` with a chat notice (never run twice, since tools may already have acted).

### Other fixes in the follow-up

- `reconcile_unconfirmed` at startup confirms `unconfirmed` posts from platform history (own messages only) on adapters with `history`.
- Slack `Retry-After` (header only) is carried through `ReqwestSend`, the legacy Slack transport and the cloud's shared-app send route.
- WhatsApp Business treated 429 as a refusal and every 503 as "not configured". It and the Discord app now rate-limit on 429 and treat only `{error: *_not_configured}` 503s as definite. Found by the new contract tests.
- The Discord app's bot-name lookup is scoped to the post's server (`find_binding_in_workspace`).
- UI (allternit-ai, all three surfaces through the shared message card): "Rate limited, not sent" state, a hover/screen-reader hint for every delivery state ("Sent" = accepted, not read), and a 429 message on file sends.

## Contract (current)

- `ChannelTransport`: `provider`, `capabilities`, `verify`, `normalize`, `identity`, `post`, `post_files`, `fetch_since`.
- `PostError`: `Rejected` (definite, nothing posted), `RateLimited { retry_after_secs }` (definite, safe to retry later), `Uncertain` (may exist; never re-posted blindly).
- `SendOutcome`: `Sent`, `Unconfirmed`, `RateLimited`, `ApprovalRequired`, `Denied`, `ReadOnly`, `NoBinding`, `Rejected`, `Replay`.
- Outbound states: `pending | confirmed | unconfirmed | failed | rate_limited | awaiting_approval | denied | drafted`. The `delivery` detail is set only by platform receipts.
- Inbound turn states: `queued | running | done | interrupted`.
- HTTP: `POST …/channel-send`, `GET …/channel`, `GET …/channel-messages/{correlationId}` (see the reference page).

## Still open

- **Live provider tests.** Every result above is from offline tests with fake platforms. Real-platform runs need authorized test accounts and destinations, and they send real messages.
- **Rollout.** The runtime change ships with the next Desktop runtime publish; the cloud Slack route deploys with cloud-api on merge; the UI deploys with allternit-ai main (ai.allternit.com) and the separate PWA deploy. Rollback = revert the merge; no schema changed (turn state lives in existing `detail_json`), so old and new builds read the same rows.
