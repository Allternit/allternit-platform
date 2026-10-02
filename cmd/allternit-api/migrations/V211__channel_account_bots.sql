-- V211: Messaging connectors (Telegram, Discord, WhatsApp, Teams) are connected
-- once and any number of bots switch them on.
--
-- channel_account_bots: which bots answer on a channel connection. A new
-- conversation opens its thread on the default bot; "@name" in a message
-- routes that message to the named bot (in a sub-thread of the conversation's
-- thread). Before this, a connection could only serve the single bot in
-- provider_account_bindings.restricted_bot_id, and the UI never set it, so
-- new conversations were dropped.
CREATE TABLE IF NOT EXISTS channel_account_bots (
    account_id  TEXT NOT NULL,
    bot_id      TEXT NOT NULL,
    owner       TEXT NOT NULL,
    is_default  INTEGER NOT NULL DEFAULT 0,
    created_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    PRIMARY KEY (account_id, bot_id)
);
CREATE INDEX IF NOT EXISTS idx_channel_account_bots_bot ON channel_account_bots(bot_id);

-- Connections restricted to one bot keep that bot, as the default.
INSERT OR IGNORE INTO channel_account_bots (account_id, bot_id, owner, is_default)
SELECT id, restricted_bot_id, owner, 1
FROM provider_account_bindings
WHERE auth_type = 'channel_oauth' AND restricted_bot_id IS NOT NULL AND restricted_bot_id <> '';
