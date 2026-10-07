-- Channel sender allowlist (audit S16).
--
-- Who may make a bot act by messaging one of its channels. Each inbound
-- binding has a policy and a list:
--   sender_policy = 'owner'  -> the owner (their verified identity on the
--                               account, or their account email for bot
--                               email) plus the senders in allowed_senders
--   sender_policy = 'anyone' -> everybody, chosen explicitly
-- allowed_senders is a JSON array of sender ids (platform user ids, email
-- addresses, phone numbers). Checked in channel_auth before any bot turn.
--
-- New bindings default to 'owner'. Existing bindings are set to 'anyone' so
-- live bots keep answering; each one gets an audit row and the app shows it
-- as "anyone can message this bot" until the owner narrows it.

ALTER TABLE provider_account_bindings ADD COLUMN sender_policy TEXT NOT NULL DEFAULT 'owner';
ALTER TABLE provider_account_bindings ADD COLUMN allowed_senders TEXT NOT NULL DEFAULT '[]';

ALTER TABLE agent_identity_channels ADD COLUMN email_sender_policy TEXT NOT NULL DEFAULT 'owner';
ALTER TABLE agent_identity_channels ADD COLUMN email_allowed_senders TEXT NOT NULL DEFAULT '[]';

ALTER TABLE slack_channel_bots ADD COLUMN sender_policy TEXT NOT NULL DEFAULT 'owner';
ALTER TABLE slack_channel_bots ADD COLUMN allowed_senders TEXT NOT NULL DEFAULT '[]';

-- Rejected senders, backfills and policy changes, per binding. Works on
-- personal installs that have no organization (audit_events requires one).
CREATE TABLE IF NOT EXISTS channel_sender_audit (
    id          TEXT PRIMARY KEY,
    channel     TEXT NOT NULL,          -- the provider (telegram, sms, slack, email, ...)
    binding     TEXT NOT NULL,          -- account id / agent id / slack channel id
    bot_id      TEXT,
    owner       TEXT,
    sender      TEXT NOT NULL,
    action      TEXT NOT NULL,          -- rejected | backfilled_open | policy_changed
    detail      TEXT,
    created_at  TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
CREATE INDEX IF NOT EXISTS idx_channel_sender_audit_binding ON channel_sender_audit(binding, created_at);

-- Backfill: everything that exists today stays open, recorded as such.
INSERT OR IGNORE INTO channel_sender_audit (id, channel, binding, bot_id, owner, sender, action, detail)
SELECT 'backfill-acct-' || id, vendor, id, restricted_bot_id, owner, '*', 'backfilled_open',
       'Connected before sender allowlists: anyone who messages this account can make the bot act. Narrow it in the account settings.'
FROM provider_account_bindings;
UPDATE provider_account_bindings SET sender_policy = 'anyone';

INSERT OR IGNORE INTO channel_sender_audit (id, channel, binding, bot_id, owner, sender, action, detail)
SELECT 'backfill-email-' || agent_id, 'email', agent_id, agent_id, user_id, '*', 'backfilled_open',
       'Bot email set up before sender allowlists: anyone who emails the bot can make it act. Narrow it in the bot''s email settings.'
FROM agent_identity_channels WHERE email_address IS NOT NULL;
UPDATE agent_identity_channels SET email_sender_policy = 'anyone' WHERE email_address IS NOT NULL;

INSERT OR IGNORE INTO channel_sender_audit (id, channel, binding, bot_id, owner, sender, action, detail)
SELECT 'backfill-slack-' || slack_channel_id, 'slack', slack_channel_id, bot_id, user_id, '*', 'backfilled_open',
       'Slack channel bound before sender allowlists: every channel member can make the bot act.'
FROM slack_channel_bots;
UPDATE slack_channel_bots SET sender_policy = 'anyone';
