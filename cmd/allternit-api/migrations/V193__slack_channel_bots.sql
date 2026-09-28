-- A Slack channel bound to a bot (spec P6.2): messages in the channel enter
-- that bot as a task thread per Slack thread, instead of the default agent.
CREATE TABLE IF NOT EXISTS slack_channel_bots (
    slack_channel_id TEXT PRIMARY KEY,
    bot_id           TEXT NOT NULL,
    user_id          TEXT NOT NULL,
    created_at       TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_slack_channel_bots_bot ON slack_channel_bots(bot_id);
