# Task (c-discord-runtime, Claude): Discord shared app, the runtime side
Read docs/CHANNELS_MAP.md, docs/CHANNELS_CONTRACTS.md (MERGE PROTOCOL), and docs/DISCORD_TASK.md.
Your scope is DISCORD_TASK's **allternit-api** part: a new `channel_discord_app.rs` with a transport that sends through the cloud `POST /api/v1/channels/discord/send` {guildId, channelId, threadId?, botName, avatarUrl?, text} → {messageId}, and a mention hook ("@Allternit name", `/name`, or a reply to a bot's message, mapped by message id). The existing user-token Discord path stays as Advanced.
Migration **V215**. Budget $12. Finish by following the MERGE PROTOCOL. Notes: docs/C_DISCORD_RUNTIME_NOTES.md.
