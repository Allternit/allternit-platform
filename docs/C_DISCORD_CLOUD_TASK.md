# Task (c-discord-cloud, Claude): Discord shared app, the cloud side
Read docs/CHANNELS_MAP.md, docs/CHANNELS_CONTRACTS.md (MERGE PROTOCOL), and docs/DISCORD_TASK.md.
Your scope is everything in DISCORD_TASK under **cloud-api**: the install link with OAuth callback, interactions, the gateway client, slash-command registration, and send through per-bot webhooks. Migration pg **025**.
The runtime side is c-discord-runtime. Your send route JSON {guildId, channelId, threadId?, botName, avatarUrl?, text} → {messageId} is the contract. Write it in your notes and the PR. Budget $15. Finish by following the MERGE PROTOCOL. Notes: docs/C_DISCORD_CLOUD_NOTES.md.
