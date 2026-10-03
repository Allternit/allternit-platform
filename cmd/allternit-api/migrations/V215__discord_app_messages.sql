-- V215: Discord shared app (runtime side). Every post made through the cloud
-- send route is recorded as message id -> bot name, so a Discord reply to one
-- of those messages is routed to the bot that wrote it. Rows older than 30
-- days are pruned as new ones are written.
CREATE TABLE IF NOT EXISTS discord_app_messages (
    message_id  TEXT PRIMARY KEY,
    channel_id  TEXT NOT NULL,
    bot_name    TEXT NOT NULL,
    created_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_discord_app_messages_created ON discord_app_messages(created_at);
