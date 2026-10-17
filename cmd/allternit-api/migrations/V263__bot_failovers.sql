-- V263: always-on failover log (agent network WP7). One row per time an
-- always-on bot's inbound messages were carried by its failover cloud computer
-- while the home runtime was offline. The settings live in agents.config
-- (`availability = {alwaysOn, failoverRuntimeId}`, `fallbackBrain`): no column.
CREATE TABLE IF NOT EXISTS bot_failovers (
    id           TEXT PRIMARY KEY,
    bot_id       TEXT NOT NULL,
    from_runtime TEXT NOT NULL,
    to_runtime   TEXT NOT NULL,
    started_at   TEXT NOT NULL,
    ended_at     TEXT,
    reason       TEXT NOT NULL DEFAULT 'home runtime offline'
);
CREATE INDEX IF NOT EXISTS idx_bot_failovers_bot ON bot_failovers(bot_id, started_at);
