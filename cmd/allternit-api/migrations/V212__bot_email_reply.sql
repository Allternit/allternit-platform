-- V212: Bot email that replies (channels wave 1). The bot's answer to an
-- inbound email is emailed back to the sender, either approval-gated
-- (mode 'approve') or straight through once the sender/domain is trusted
-- (modes 'auto_known' / 'auto', the latter only on a verified domain).
--
-- agent_identity_channels gains the per-bot reply policy; agent_email_inbound
-- stores the threading headers the reply needs plus why a message was skipped
-- (loop guard) and what the reply did; agent_email_outbound marks rows that
-- are bot replies so the reply caps can count them.

ALTER TABLE agent_identity_channels ADD COLUMN email_reply_mode TEXT NOT NULL DEFAULT 'approve';
ALTER TABLE agent_identity_channels ADD COLUMN email_reply_allowlist TEXT;
ALTER TABLE agent_identity_channels ADD COLUMN email_reply_enabled INTEGER NOT NULL DEFAULT 1;
-- The mailbox's domain passed outbound verification; only then may a bot be
-- switched to reply mode 'auto'. Nothing sets this to true yet besides tests.
ALTER TABLE agent_identity_channels ADD COLUMN email_domain_verified INTEGER NOT NULL DEFAULT 0;

ALTER TABLE agent_email_inbound ADD COLUMN in_reply_to TEXT;
ALTER TABLE agent_email_inbound ADD COLUMN email_references TEXT;
ALTER TABLE agent_email_inbound ADD COLUMN reply_to TEXT;
ALTER TABLE agent_email_inbound ADD COLUMN auth_results TEXT;
-- Loop-guard outcome: NULL means the turn ran; otherwise the reason it didn't.
ALTER TABLE agent_email_inbound ADD COLUMN guard_reason TEXT;
-- What the reply did: 'sent' | 'pending_approval' | 'failed' | 'skipped'.
ALTER TABLE agent_email_inbound ADD COLUMN reply_status TEXT;

-- Set on outbound rows that are the bot's reply to an inbound email; the
-- reply caps count on it.
ALTER TABLE agent_email_outbound ADD COLUMN reply_inbound_id TEXT;
CREATE INDEX IF NOT EXISTS idx_agent_email_outbound_reply ON agent_email_outbound(reply_inbound_id);
