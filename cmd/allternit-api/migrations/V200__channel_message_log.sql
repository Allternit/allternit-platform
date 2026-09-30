-- Allternit Agent Gateway: Channel Packs message ledger. Additive only.
--
-- One row per inbound event (message, edit, delete, reaction change) and per
-- outbound post of a channel_conversation_bindings row. remote_id is the
-- platform's stable id (Slack ts, ...) and is what dedupes replays;
-- correlation_id dedupes outbound retries. state for outbound rows:
-- pending | confirmed | unconfirmed | failed | awaiting_approval.
CREATE TABLE IF NOT EXISTS channel_message_log (
    id             TEXT PRIMARY KEY,
    owner          TEXT NOT NULL,
    binding_id     TEXT NOT NULL,
    thread_id      TEXT NOT NULL,
    direction      TEXT NOT NULL,
    kind           TEXT NOT NULL,
    remote_id      TEXT,
    correlation_id TEXT,
    state          TEXT NOT NULL DEFAULT 'confirmed',
    detail_json    TEXT NOT NULL DEFAULT '{}',
    created_at     TEXT NOT NULL,
    updated_at     TEXT NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS idx_channel_message_log_remote ON channel_message_log(binding_id, direction, remote_id) WHERE remote_id IS NOT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS idx_channel_message_log_corr ON channel_message_log(binding_id, direction, correlation_id) WHERE correlation_id IS NOT NULL;
CREATE INDEX IF NOT EXISTS idx_channel_message_log_thread ON channel_message_log(thread_id);
