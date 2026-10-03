# Task (ao-discord): Discord shared app (platform)
Read CHANNELS_MAP.md and CHANNELS_CONTRACTS.md. Migrations: V215 and 025.

Goal: "Add to Discord" → pick a server → Authorize. Allternit's one Discord application reads and writes there. Every Allternit bot speaks with its own name and avatar. Nothing is pasted.

## Build

### cloud-api, new `channels/discord_app.rs`
- Env: `ALLTERNIT_DISCORD_APP_ID`, `_PUBLIC_KEY`, `_BOT_TOKEN`, `_CLIENT_SECRET`.
- **Install link**
  - `POST /api/v1/channels/discord/install` {runtimeId} returns {url} with:
    - scopes `bot applications.commands identify`;
    - `integration_type=0`;
    - permissions: View Channels, Send Messages, Send in Threads, Create Public Threads, Read History, Embed Links, Attach Files, Add Reactions, Manage Webhooks. Compute the integer and cite the docs.
    - a signed `state`.
  - The callback `GET /channels/discord/oauth/callback` stores guild → user/runtime (pg 025 `discord_installs`), creates the relay inbound route, and shows a "Connected, you can close this" page that deep-links back into the app.
- **Interactions** `POST /channels/discord/interactions`: Ed25519 verify, then answer PING type 1 instantly. Slash commands → deferred response → queue to the runtime.
- **Gateway** in the cloud: one WebSocket client (tokio-tungstenite if it's already in the tree; otherwise justify) with intents GUILDS, GUILD_MESSAGES and DIRECT_MESSAGES, **without** MESSAGE_CONTENT. Route MESSAGE_CREATE events that mention the app, reply to its messages, or come from DMs to the owner runtime by guild. Handle resume and reconnect. Leave a TODO for sharding past 2,500 guilds.
- **Slash commands**: register guild commands `/<botname>` for each bot switched on. A runtime call to `PUT /api/v1/channels/discord/commands` sends the list of names.
- **Send**: `POST /api/v1/channels/discord/send` posts through an app-owned **webhook** per channel (create it lazily), with `username` and `avatar_url` per bot and `thread_id` for threads. Never use "discord" or "clyde" in names.

### allternit-api, new `channel_discord_app.rs`
- A transport whose post goes through the cloud send route.
- Mention hook: "@Allternit <name>", `/name`, or a reply to a message a bot sent (map message id → bot).
- The existing user-token Discord path stays as "Advanced".

## Tests
With fakes:
- install link fields;
- OAuth callback stores the install;
- interactions PING and a bad signature;
- a gateway MESSAGE_CREATE with a mention is relayed, and one without a mention is ignored;
- webhook send carries the per-bot username;
- the mention hook picks the right bot.

## Notes
`docs/DISCORD_NOTES.md`, then the sentinel.
