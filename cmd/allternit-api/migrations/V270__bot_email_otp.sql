-- V270: Bot inbox reading and one-time codes. Inbound bot email stores what
-- `email_otp::extract` found at insert time (codes and magic links as JSON
-- arrays, and whether the message is a verification email). Verification
-- mail never starts a bot turn; the bot reads it with the allternit_mail.read_otp
-- tool. read_at marks a message the bot has opened (allternit_mail.read).

ALTER TABLE agent_email_inbound ADD COLUMN otp_codes TEXT;
ALTER TABLE agent_email_inbound ADD COLUMN otp_links TEXT;
ALTER TABLE agent_email_inbound ADD COLUMN is_verification INTEGER NOT NULL DEFAULT 0;
ALTER TABLE agent_email_inbound ADD COLUMN read_at DATETIME;
CREATE INDEX IF NOT EXISTS idx_agent_email_inbound_agent_created ON agent_email_inbound(agent_id, created_at);
