# c-discord-cloud notes: Discord shared app, cloud side

Executor: c-discord-cloud (Claude). Orchestrator: joe-07. Migration: cloud-api pg **025**.

## Files changed

- `cmd/allternit-cloud-api/src/routes/discord_app.rs` (new, owns everything): config, Discord HTTP seam, install link and signed state, OAuth callback, interactions, gateway client, slash-command registration, webhook send, tests.
- `cmd/allternit-cloud-api/migrations_pg/025_discord_installs.sql` (new): `discord_installs`, `discord_webhooks`.
- `routes/channel_inbound.rs`: `target_path("discord_app")` arm; `sha256_hex`, `new_key`, `public_base`, `deliver_route` made `pub(crate)` (no behavior change).
- `routes/mod.rs`, `lib.rs`, `main.rs`: one line each (module, router merge, gateway start).
- `Cargo.toml`: `tokio-tungstenite` gets `rustls-tls-webpki-roots`. It was already in the tree (0.21) but without TLS, and the Discord gateway is `wss://` only, so the client cannot connect without it.

## Env (cloud-api only; all four required, otherwise every route is 503 `{"error":"discord_not_configured"}` and the gateway never starts)

`ALLTERNIT_DISCORD_APP_ID`, `ALLTERNIT_DISCORD_PUBLIC_KEY`, `ALLTERNIT_DISCORD_BOT_TOKEN`, `ALLTERNIT_DISCORD_CLIENT_SECRET`.

Discord developer portal settings to match:
- OAuth2 redirect: `<ALLTERNIT_CLOUD_API_URL or https://api.allternit.com>/channels/discord/oauth/callback`
- Interactions endpoint URL: `https://api.allternit.com/channels/discord/interactions`
- Bot: no privileged intents needed (MESSAGE_CONTENT is not requested).

## Endpoint JSON

User routes authenticate like `channel-inbound-routes` (Clerk / device token, scope `compute`).

- `POST /api/v1/channels/discord/install` `{"runtimeId":"rt_…"}` → `{"url":"https://discord.com/oauth2/authorize?client_id=…&scope=bot+applications.commands+identify&permissions=309774634048&integration_type=0&response_type=code&redirect_uri=…&state=…"}`. 404 if the runtime is not the caller's.
- `GET /channels/discord/oauth/callback?code&state&guild_id` (public): HTML page ("Connected", deep link `allternit://channels/discord/connected`). Errors render an HTML page with 400/502.
- `POST /channels/discord/interactions` (public, Ed25519): PING → `{"type":1}`; slash command → `{"type":5,"data":{"flags":64}}` (deferred, ephemeral) and the event is queued; unknown server → type 4 ephemeral notice; bad signature → 401.
- `PUT /api/v1/channels/discord/commands` `{"guildId":"…","names":["helper","scout"]}` → `{"registered":["helper","scout"]}` (names lowercased to `a-z0-9_-`, max 32 chars, deduped, "discord"/"clyde" removed, max 100). Each command is `/<name> message:<text>`. 404 `discord_not_installed` if the caller has no active install of that guild.
- **Send (contract for c-discord-runtime):** `POST /api/v1/channels/discord/send`
  - request `{"guildId":"…","channelId":"…","threadId":"…"?,"botName":"…","avatarUrl":"https://…"?,"text":"…"}`
  - response `{"messageId":"…","messageIds":["…"]}`. `messageId` is the first message; long text is split at 2,000 characters and `messageIds` lists every part (map any of them to the bot for reply routing).
  - `channelId` is the **parent** channel when `threadId` is set (inbound envelopes already give it that way).
  - Errors: 404 `discord_not_installed` (caller has no install for `guildId`), 403 `channel_not_in_guild`, 400 `empty_text`, 502 `discord_error` `{status}` / `discord_unreachable`, 503 `discord_not_configured`.
  - `username` is the bot name with "discord"/"clyde" removed (empty → "Allternit"), `avatar_url` only if https, `allowed_mentions.parse = []` so a bot can never ping @everyone.

## Inbound contract for c-discord-runtime (needs to match)

Events are queued in `channel_inbound_queue` under a route with provider **`discord_app`** and delivered by the existing worker to the runtime path **`POST /webhooks/channels/discord-app`** (new `target_path` arm). Using its own path and provider means a raw user-token Discord payload can never be mistaken for a trusted envelope. The runtime does not verify any Discord signature here: cloud already did. Body (JSON, camelCase):

```json
{ "source": "allternit-discord-app", "kind": "message" | "command",
  "guildId": "…" | null, "channelId": "…", "threadId": "…" | null,
  "messageId": "…" | null, "authorId": "…", "authorName": "…",
  "content": "text with the app mention removed", "isDm": false, "mentionsApp": true,
  "replyToMessageId": "…" | null, "commandName": "helper" | null, "interactionId": "…" | null,
  "attachments": [{ "url": "…", "filename": "…", "contentType": "…" }] }
```

- `message`: gateway MESSAGE_CREATE that mentions the app, replies to a message the app/its webhook posted, or is a DM to the Discord user who installed. A reply with the ping turned off carries no text (Discord withholds content without the intent): the runtime should treat empty `content` + `replyToMessageId` as "continue that bot's thread".
- `command`: slash command; `commandName` is the bot's `/name`, `content` is the `message` option. The runtime posts the answer through the send route; cloud already edits the ephemeral placeholder to "Asked <name>. Its reply will appear here."
- "@Allternit <name>" is not parsed in cloud: it arrives as `content` for the runtime's mention hook (the app mention is stripped, so `content` starts with `<name>`).
- Messages from bots, webhooks and the app itself are dropped in cloud. The runtime still dedupes by `messageId`.

## Design notes

- **Gateway**: tokio-tungstenite over rustls. Intents GUILDS|GUILD_MESSAGES|DIRECT_MESSAGES = 4609 (no MESSAGE_CONTENT). Identify, heartbeat with ACK tracking, resume (op 6 with saved session/seq/resume URL), reconnect (op 7), invalid-session (op 9), backoff, stop on fatal close codes (4004, 4010–4014). One replica holds it via a Postgres session advisory lock (key `ALTDISCD`); others retry every 30 s. `GUILD_DELETE` (not an outage) revokes the install. `TODO(sharding)` is in `identify` for 2,500+ guilds.
- **Webhooks**: created lazily per channel; only the webhook id is stored (`discord_webhooks`), the token is re-read with the bot token and cached in memory, so no webhook secret sits in Postgres. The channel must belong to the install's guild (checked against Discord and the stored row), so one user's install cannot post into another server. A deleted webhook (10015/50027) is replaced once; a 429 is retried once.
- **Install state**: `base64url(user|runtime|exp|nonce).hmac`, keyed from the client secret, 15-minute expiry. The callback re-checks the runtime still belongs to the user. Last install of a guild wins (installing needs Manage Server in Discord). A reinstall for the same runtime reuses the relay route.
- **Relay route key**: created and immediately dropped, so the public `/channels/in/<key>` address for an install can never be used; events are queued internally.
- **DMs**: routed to the most recent active install whose installer's Discord id matches the sender.

## Docs verified (2026-10-02, from the official docs)

- OAuth2 / bot scope, `permissions`, `integration_type`, token response `guild`: https://discord.com/developers/docs/topics/oauth2
- Permission bits (View Channel 1<<10, Send 1<<11, Embed 1<<14, Attach 1<<15, History 1<<16, Add Reactions 1<<6, Manage Webhooks 1<<29, Create Public Threads 1<<35, Send in Threads 1<<38): https://discord.com/developers/docs/topics/permissions
- Gateway, intents, identify/resume/heartbeat, close codes: https://discord.com/developers/docs/events/gateway
- Interactions, PING, signature headers, deferred type 5, `@original` edit: https://discord.com/developers/docs/interactions/receiving-and-responding
- Webhook execute (`username`, `avatar_url`, `thread_id`, `wait`), create channel webhook: https://discord.com/developers/docs/resources/webhook
- Bulk overwrite guild commands: https://discord.com/developers/docs/interactions/application-commands

Docs moved to https://docs.discord.com/developers/…; I fetched the OAuth2 and webhook pages: redirect carries `guild_id` + `permissions`, `integration_type` 0 = GUILD_INSTALL, `wait`/`thread_id`/`username`/`avatar_url` confirmed, and webhook names may not contain "clyde"/"discord". **Unconfirmed:** whether `GET /webhooks/{id}` with the bot token returns `token` for an app-owned webhook (the page is ambiguous). The code handles both: with no token it deletes that webhook and creates a new one (one extra webhook per channel per process restart in the worst case). If Discord does not return it, switch to storing the token sealed with `credential_cipher`. A live check against a test app is the first thing to do when credentials exist.

## Tests

`CARGO_TARGET_DIR=/Users/joe/Desktop/allternit-workspace/.gw-target cargo test -p allternit-cloud-api --lib discord_app` → 16 passed (fake Discord HTTP, schema-per-test Postgres with migrations 020 + 025): permissions integer, install link fields and state (tamper/expiry/other secret), install needs an owned runtime, OAuth callback stores install + route (and refuses bad state, denial, foreign runtime), PING and five bad-signature cases, slash command deferred and queued, gateway mention relayed / no-mention ignored / reply / DM / thread / bot and webhook skipped, gateway frame state machine (identify, heartbeat, resume, invalid session), command registration, send carries per-bot username and avatar, wrong-guild refused, deleted webhook replaced, name/text sanitising.

Gate results are recorded below by the merge step.

## Prod steps (by hand)

1. Apply `migrations_pg/025_discord_installs.sql` to the prod cloud database (cloud-api also applies embedded migrations at boot unless `ALLTERNIT_SKIP_MIGRATIONS` is set).
2. Create the Discord application, set the redirect and interactions URLs above, set the four env vars on cloud-api, restart. Until then everything is 503/inert.
3. allternit-api runtime side (c-discord-runtime) must implement `/webhooks/channels/discord-app` and call the send route above.

## What's left

- allternit-ai wizard button ("Add to Discord" → `install` → open URL) and call to `PUT …/commands` when a bot is switched on/off: owned by the UI and runtime executors.
- Live check against a real Discord test application (no credentials were used here).
- Sharding past 2,500 guilds; gateway failover on lock-connection loss is best effort (the loop re-checks the lock connection on each reconnect).
- No allternit-ai files changed, so the allternit-ai vitest/tsc gate does not apply to this PR.
