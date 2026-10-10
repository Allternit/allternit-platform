ALTER TABLE platform_events ADD COLUMN IF NOT EXISTS mcp_sequence BIGSERIAL;
CREATE INDEX IF NOT EXISTS platform_events_mcp_replay ON platform_events(user_id,mcp_sequence) WHERE subject='user';
ALTER TABLE platform_webhook_deliveries ADD COLUMN IF NOT EXISTS replay_started_at TIMESTAMPTZ;
